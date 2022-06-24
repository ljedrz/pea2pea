mod common;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt, TryStreamExt};
use libp2p::swarm::Swarm;
use libp2p::{identity, ping, PeerId, Transport};
use parking_lot::Mutex;
use prost::Message;
use tokio::time::sleep;
use tokio_util::codec::{Decoder, Encoder, Framed, FramedParts, LengthDelimitedCodec};
use tracing::*;
use tracing_subscriber::filter::LevelFilter;
use unsigned_varint::codec::UviBytes;

use pea2pea::{
    protocols::{Handshake, Reading, Writing},
    Connection, ConnectionSide, Node, Pea2Pea,
};

use std::{
    collections::{HashMap, HashSet},
    fmt, io,
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};

// payloads for Noise handshake messages
// note: this struct was auto-generated using prost-build based on
// the proto file from the libp2p-noise repository
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct NoiseHandshakePayload {
    #[prost(bytes = "vec", tag = "1")]
    pub identity_key: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub identity_sig: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    pub data: ::prost::alloc::vec::Vec<u8>,
}

// maximum noise message size, as specified by its protocol
const NOISE_MAX_LEN: usize = 65535;

// the version used in yamux message headers
const YAMUX_VERSION: u8 = 0;

// the default libp2p ping interval
const PING_INTERVAL_SECS: u64 = 5;

// the libp2p-cabable node
#[derive(Clone)]
struct Libp2pNode {
    // the pea2pea node
    node: Node,
    // the libp2p keypair
    keypair: identity::Keypair,
    // holds noise states between the handshake and the other protocols
    noise_states: Arc<Mutex<HashMap<SocketAddr, NoiseState>>>,
    // holds the state related to peers
    peer_states: Arc<Mutex<HashMap<PeerId, PeerState>>>,
}

impl Pea2Pea for Libp2pNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

#[derive(Default)]
struct PeerState;

// an object representing the state of noise
enum NoiseState {
    Handshake(Box<snow::HandshakeState>),
    PostHandshake {
        // stateless state can be used immutably
        state: Arc<snow::StatelessTransportState>,
        // only used in Reading
        rx_nonce: Option<u64>,
        // only used in Writing
        tx_nonce: Option<u64>,
    },
}

// only post-handshake state is cloned (in the Reading impl)
impl Clone for NoiseState {
    fn clone(&self) -> Self {
        match self {
            Self::Handshake(..) => panic!("unsupported"),
            Self::PostHandshake {
                state,
                rx_nonce,
                tx_nonce,
            } => Self::PostHandshake {
                state: Arc::clone(state),
                rx_nonce: *rx_nonce,
                tx_nonce: *tx_nonce,
            },
        }
    }
}

impl NoiseState {
    // obtain the handshake noise state
    fn handshake(&mut self) -> Option<&mut snow::HandshakeState> {
        if let Self::Handshake(state) = self {
            Some(state)
        } else {
            None
        }
    }

    // transform the noise state into post-handshake
    fn into_post_handshake(self) -> Self {
        if let Self::Handshake(state) = self {
            let state = state.into_stateless_transport_mode().unwrap();
            Self::PostHandshake {
                state: Arc::new(state),
                rx_nonce: Some(0),
                tx_nonce: Some(0),
            }
        } else {
            panic!();
        }
    }

    // obtain the post-handshake noise state
    fn post_handshake(&self) -> &Arc<snow::StatelessTransportState> {
        if let Self::PostHandshake { state, .. } = self {
            state
        } else {
            panic!();
        }
    }

    // mutably borrow the receive nonce
    fn rx_nonce(&mut self) -> &mut u64 {
        if let Self::PostHandshake {
            rx_nonce: Some(ref mut val),
            ..
        } = self
        {
            val
        } else {
            panic!();
        }
    }

    // mutably borrow the send nonce
    fn tx_nonce(&mut self) -> &mut u64 {
        if let Self::PostHandshake {
            tx_nonce: Some(ref mut val),
            ..
        } = self
        {
            val
        } else {
            panic!();
        }
    }
}

// a codec used to (en/de)crypt messages using noise
struct NoiseCodec {
    codec: LengthDelimitedCodec,
    noise: NoiseState,
    buffer: Box<[u8]>,
}

impl NoiseCodec {
    fn new(noise: NoiseState) -> Self {
        NoiseCodec {
            codec: LengthDelimitedCodec::builder()
                .length_field_length(2)
                .new_codec(),
            noise,
            buffer: vec![0u8; NOISE_MAX_LEN].into(),
        }
    }
}

impl Decoder for NoiseCodec {
    type Item = Bytes;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // obtain the whole message first, using the length-delimited codec
        let bytes = if let Some(bytes) = self.codec.decode(src)? {
            bytes
        } else {
            return Ok(None);
        };

        // decrypt it in the handshake or post-handshake mode
        let msg_len = if let Some(noise) = self.noise.handshake() {
            noise
                .read_message(&bytes, &mut self.buffer)
                .map_err(|_| io::ErrorKind::InvalidData)?
        } else {
            let rx_nonce = *self.noise.rx_nonce();
            let len = self
                .noise
                .post_handshake()
                .read_message(rx_nonce, &bytes, &mut self.buffer)
                .map_err(|_| io::ErrorKind::InvalidData)?;
            *self.noise.rx_nonce() += 1;
            len
        };
        let msg = self.buffer[..msg_len].to_vec().into();

        Ok(Some(msg))
    }
}

impl Encoder<Bytes> for NoiseCodec {
    type Error = io::Error;

    fn encode(&mut self, msg: Bytes, dst: &mut BytesMut) -> Result<(), Self::Error> {
        // encrypt the message in the handshake or post-handshake mode
        let msg_len = if let Some(noise) = self.noise.handshake() {
            noise.write_message(&msg, &mut self.buffer).unwrap()
        } else {
            let tx_nonce = *self.noise.tx_nonce();
            let len = self
                .noise
                .post_handshake()
                .write_message(tx_nonce, &msg, &mut self.buffer)
                .map_err(|_| io::ErrorKind::InvalidData)?;
            *self.noise.tx_nonce() += 1;
            len
        };
        let msg: Bytes = self.buffer[..msg_len].to_vec().into();

        // encode it using the length-delimited codec
        self.codec.encode(msg, dst)
    }
}

// the numeric ID of a yamux stream
type StreamId = u32;

// a set of yamux streams belonging to a single connection
type Streams = HashSet<StreamId>;

// a header describing a yamux message
#[derive(Clone, PartialEq, Eq)]
struct YamuxHeader {
    version: u8,
    ty: YamuxType,
    flags: Vec<YamuxFlag>,
    stream_id: StreamId,
    length: u32,
}

impl fmt::Debug for YamuxHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // note: the hardcoded version is hidden for brevity
        write!(
            f,
            "{{ StreamID: {}, Type: {}, Flags: {:?}, Length: {} }}",
            self.stream_id, self.ty, self.flags, self.length
        )
    }
}

// custom events that can trigger actions on pea2pea side
#[derive(Debug, Clone, PartialEq, Eq)]
enum YamuxEvent {
    NewStream(StreamId),
    StreamTerminated(StreamId),
}

// a full yamux message
#[derive(Debug, Clone, PartialEq, Eq)]
struct YamuxMessage {
    header: YamuxHeader,
    payload: Bytes,
    // an optional internal event related to a yamux message
    event: Option<YamuxEvent>,
}

impl YamuxMessage {
    // creates a new data message for the given yamux stream ID
    fn data(stream_id: u32, flags: Vec<YamuxFlag>, data: Option<Bytes>) -> Self {
        let payload = data.unwrap_or_default();

        let header = YamuxHeader {
            version: YAMUX_VERSION,
            ty: YamuxType::Data,
            flags,
            stream_id,
            length: payload.len() as u32,
        };

        Self {
            header,
            payload,
            event: None,
        }
    }

    fn terminate(stream_id: u32) -> Self {
        let header = YamuxHeader {
            version: YAMUX_VERSION,
            ty: YamuxType::GoAway,
            flags: vec![],
            stream_id,
            length: YamuxTermination::Normal as u32,
        };

        Self {
            header,
            payload: Default::default(),
            event: None,
        }
    }
}

// a codec used to (en/de)crypt noise messages and interpret them as yamux ones
struct YamuxCodec {
    // the underlying noise codec
    codec: NoiseCodec,
    // client or server
    #[allow(dead_code)] // TODO: use it in message creation
    mode: YamuxMode,
    // the yamux streams applicable to a connection
    // note: they are within the codec (as opposed to global node state)
    // for performance reasons
    streams: Streams,
    // the node's tracing span
    span: Span,
}

impl YamuxCodec {
    fn new(codec: NoiseCodec, side: ConnectionSide, span: Span) -> Self {
        let mode = if side == ConnectionSide::Initiator {
            YamuxMode::Client
        } else {
            YamuxMode::Server
        };

        Self {
            codec,
            mode,
            streams: Default::default(),
            span,
        }
    }
}

// indicates the type of a yamux message
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum YamuxType {
    // used to transmit data
    Data = 0x0,
    // used to update the sender's receive window size
    WindowUpdate = 0x1,
    // used to measure RTT
    Ping = 0x2,
    // used to close a session
    GoAway = 0x3,
}

impl fmt::Display for YamuxType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Data => write!(f, "Data"),
            Self::WindowUpdate => write!(f, "Window Update"),
            Self::Ping => write!(f, "Ping"),
            Self::GoAway => write!(f, "Go Away"),
        }
    }
}

impl TryFrom<u8> for YamuxType {
    type Error = io::Error;

    fn try_from(ty: u8) -> io::Result<Self> {
        match ty {
            0x0 => Ok(Self::Data),
            0x1 => Ok(Self::WindowUpdate),
            0x2 => Ok(Self::Ping),
            0x3 => Ok(Self::GoAway),
            _ => Err(io::ErrorKind::InvalidData.into()),
        }
    }
}

// indicates the termination of a session
#[repr(u32)]
enum YamuxTermination {
    Normal = 0,
    #[allow(dead_code)]
    ProtocolError = 1,
    #[allow(dead_code)]
    InternalError = 2,
}

// additional information related to the yamux message type
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum YamuxFlag {
    // signals the start of a new stream
    Syn = 0x1,
    // acknowledges the start of a new stream
    Ack = 0x2,
    // performs a half-close of a stream
    Fin = 0x4,
    // resets a stream immediately
    Rst = 0x8,
}

impl fmt::Display for YamuxFlag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syn => write!(f, "SYN"),
            Self::Ack => write!(f, "ACK"),
            Self::Fin => write!(f, "FIN"),
            Self::Rst => write!(f, "RST"),
        }
    }
}

impl fmt::Debug for YamuxFlag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self)
    }
}

impl TryFrom<u8> for YamuxFlag {
    type Error = io::Error;

    fn try_from(flag: u8) -> io::Result<Self> {
        match flag {
            0x1 => Ok(Self::Syn),
            0x2 => Ok(Self::Ack),
            0x4 => Ok(Self::Fin),
            0x8 => Ok(Self::Rst),
            _ => Err(io::ErrorKind::InvalidData.into()),
        }
    }
}

// interpret the flags encoded in a yamux message
fn decode_flags(flags: u16) -> io::Result<Vec<YamuxFlag>> {
    let mut ret = Vec::new();

    for n in 0..15 {
        let bit = 1 << n;
        if flags & bit != 0 {
            ret.push(YamuxFlag::try_from(bit as u8)?);
        }
    }

    Ok(ret)
}

// encode the given flags in a yamux message
fn encode_flags(flags: &[YamuxFlag]) -> u16 {
    let mut ret = 0u16;

    for flag in flags {
        ret |= *flag as u16;
    }

    ret
}

// the side of a yamux connection
#[derive(Clone, Copy, PartialEq, Eq)]
enum YamuxMode {
    // client side should use odd stream IDs
    Client,
    // server side should use even stream IDs
    Server,
}

impl Decoder for YamuxCodec {
    type Item = YamuxMessage;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // decrypt a noise message
        let mut bytes = if let Some(bytes) = self.codec.decode(src)? {
            bytes
        } else {
            return Ok(None);
        };

        // decode the yamux message
        let version = bytes.get_u8();
        let ty = YamuxType::try_from(bytes.get_u8())?;
        let flags = decode_flags(bytes.get_u16())?;
        let stream_id = bytes.get_u32();
        let length = bytes.get_u32();

        // register some custom events so they can later be acted on
        let event = if flags == [YamuxFlag::Syn] {
            // a new stream is being created
            if !self.streams.insert(stream_id) {
                error!(parent: &self.span, "yamux stream {} had already been registered", stream_id);
                return Err(io::ErrorKind::InvalidData.into());
            } else {
                Some(YamuxEvent::NewStream(stream_id))
            }
        } else if flags == [YamuxFlag::Rst] {
            // a stream is being terminated
            if !self.streams.remove(&stream_id) {
                error!(parent: &self.span, "yamux stream {} is unknown", stream_id);
                return Err(io::ErrorKind::InvalidData.into());
            } else {
                Some(YamuxEvent::StreamTerminated(stream_id))
            }
        } else {
            None
        };

        // construct the header
        let header = YamuxHeader {
            version,
            ty,
            flags,
            stream_id,
            length,
        };

        // return the full message
        Ok(Some(YamuxMessage {
            header,
            payload: bytes.split_to(length as usize),
            event,
        }))
    }
}

impl Encoder<YamuxMessage> for YamuxCodec {
    type Error = io::Error;

    fn encode(&mut self, msg: YamuxMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
        // prepare the serialized yamux message
        let mut bytes = BytesMut::new();

        // version
        bytes.put_u8(msg.header.version);
        // type
        bytes.put_u8(msg.header.ty as u8);
        // flags
        bytes.put_u16(encode_flags(&msg.header.flags));
        // stream ID
        bytes.put_u32(msg.header.stream_id);
        // length
        bytes.put_u32(msg.payload.len() as u32);

        // data
        bytes.put(msg.payload);

        // encrypt the message with the underlying noise codec
        self.codec.encode(bytes.freeze(), dst)
    }
}

#[async_trait::async_trait]
impl Handshake for Libp2pNode {
    const TIMEOUT_MS: u64 = 5_000;

    async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
        // the noise handshake settings
        const HANDSHAKE_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_SHA256";

        // create the noise objects
        let builder = snow::Builder::new(HANDSHAKE_PATTERN.parse().unwrap());
        let noise_keypair = builder.generate_keypair().unwrap();
        let noise_builder = builder.local_private_key(&noise_keypair.private);

        let node_conn_side = !conn.side();
        let addr = conn.addr();
        let stream = self.borrow_stream(&mut conn);

        let (framed, peer_id) = match node_conn_side {
            ConnectionSide::Initiator => {
                let mut negotiation_codec = Framed::new(stream, UviBytes::default());

                // -> protocol info (1/2)
                negotiation_codec
                    .send(Bytes::from("/multistream/1.0.0\n"))
                    .await?;
                debug!(parent: self.node().span(), "sent protocol params (1/2)");

                // <- protocol info (1/2)
                let _protocol_info = negotiation_codec
                    .try_next()
                    .await?
                    .ok_or(io::ErrorKind::InvalidData)?;
                debug!(parent: self.node().span(), "received protocol params (1/2)");

                // -> protocol info (2/2)
                negotiation_codec.send(Bytes::from("/noise\n")).await?;
                debug!(parent: self.node().span(), "sent protocol params (2/2)");

                // <- protocol info (2/2)
                let _protocol_info = negotiation_codec
                    .try_next()
                    .await?
                    .ok_or(io::ErrorKind::InvalidData)?;
                debug!(parent: self.node().span(), "received protocol params (1/2)");

                // build the Noise codec
                let stream = negotiation_codec.into_inner();
                let noise = Box::new(noise_builder.build_initiator().unwrap());
                let mut framed = Framed::new(stream, NoiseCodec::new(NoiseState::Handshake(noise)));

                // -> e
                framed.send("".into()).await?;
                debug!(parent: self.node().span(), "sent e (XX handshake 1/3)");

                // <- e, ee, s, es + the payload
                let handshake_payload_raw =
                    framed.try_next().await?.ok_or(io::ErrorKind::InvalidData)?;
                debug!(parent: self.node().span(), "received e, ee, s, es (XX handshake 2/3)");
                let handshake_payload = NoiseHandshakePayload::decode(&handshake_payload_raw[..])?;
                let peek_key =
                    identity::PublicKey::from_protobuf_encoding(&handshake_payload.identity_key)
                        .map_err(|_| io::ErrorKind::InvalidData)?;
                let peer_id = PeerId::from(peek_key);
                info!(parent: self.node().span(), "the peer ID of {} is {}", addr, &peer_id);

                // -> s, se + the payload
                let pb = NoiseHandshakePayload {
                    identity_key: self.keypair.public().to_protobuf_encoding(),
                    identity_sig: self
                        .keypair
                        .sign(&[&b"noise-libp2p-static-key:"[..], &noise_keypair.public].concat())
                        .unwrap(),
                    data: vec![],
                };
                let mut msg = Vec::with_capacity(pb.encoded_len());
                pb.encode(&mut msg).unwrap();
                framed.send(msg.into()).await?;
                debug!(parent: self.node().span(), "sent s, se (XX handshake 3/3)");

                // upgrade noise state to post-handshake
                let FramedParts { codec, .. } = framed.into_parts();
                let NoiseCodec { noise, .. } = codec;
                let noise = noise.into_post_handshake();

                // reconstruct the Framed with the post-handshake noise state
                let mut framed = Framed::new(self.borrow_stream(&mut conn), NoiseCodec::new(noise));

                // -> protocol info (1/2)
                framed
                    .send(Bytes::from(&b"\x13/multistream/1.0.0\n"[..]))
                    .await?;
                debug!(parent: self.node().span(), "sent protocol params (1/2)");

                // <- protocol info (1/2)
                let _protocol_info = framed.try_next().await?.ok_or(io::ErrorKind::InvalidData)?;
                debug!(parent: self.node().span(), "received protocol params (1/2)");

                // -> protocol info (2/2)
                framed.send(Bytes::from(&b"\r/yamux/1.0.0\n"[..])).await?;
                debug!(parent: self.node().span(), "sent protocol params (2/2)");

                // <- protocol info (2/2)
                let _protocol_info = framed.try_next().await?.ok_or(io::ErrorKind::InvalidData)?;
                debug!(parent: self.node().span(), "received protocol params (2/2)");

                (framed, peer_id)
            }
            ConnectionSide::Responder => {
                let mut negotiation_codec = Framed::new(stream, UviBytes::default());

                // <- protocol info (1/2)
                let _protocol_info = negotiation_codec.try_next().await?;
                debug!(parent: self.node().span(), "received protocol params (1/2)");

                // -> protocol info (1/2)
                negotiation_codec
                    .send(Bytes::from("/multistream/1.0.0\n"))
                    .await?;
                debug!(parent: self.node().span(), "sent protocol params (1/2)");

                // <- protocol info (2/2)
                let _protocol_info = negotiation_codec.try_next().await?;
                debug!(parent: self.node().span(), "received protocol params (2/2)");

                // -> protocol info (2/2)
                negotiation_codec.send(Bytes::from("/noise\n")).await?;
                debug!(parent: self.node().span(), "sent protocol params (2/2)");

                // build the Noise codec
                let stream = negotiation_codec.into_inner();
                let noise = Box::new(noise_builder.build_responder().unwrap());
                let mut framed = Framed::new(stream, NoiseCodec::new(NoiseState::Handshake(noise)));

                // <- e
                framed.try_next().await?.ok_or(io::ErrorKind::InvalidData)?;
                debug!(parent: self.node().span(), "received e (XX handshake 1/3)");

                // -> e, ee, s, es + the payload
                let pb = NoiseHandshakePayload {
                    identity_key: self.keypair.public().to_protobuf_encoding(),
                    identity_sig: self
                        .keypair
                        .sign(&[&b"noise-libp2p-static-key:"[..], &noise_keypair.public].concat())
                        .unwrap(),
                    data: vec![],
                };
                let mut msg = Vec::with_capacity(pb.encoded_len());
                pb.encode(&mut msg).unwrap();
                framed.send(msg.into()).await?;
                debug!(parent: self.node().span(), "sent e, ee, s, es (XX handshake 2/3)");

                // <- s, se + the payload
                let handshake_payload_raw =
                    framed.try_next().await?.ok_or(io::ErrorKind::InvalidData)?;
                debug!(parent: self.node().span(), "received s, se, psk (XX handshake 3/3)");
                let handshake_payload = NoiseHandshakePayload::decode(&handshake_payload_raw[..])?;
                let peek_key =
                    identity::PublicKey::from_protobuf_encoding(&handshake_payload.identity_key)
                        .map_err(|_| io::ErrorKind::InvalidData)?;
                let peer_id = PeerId::from(peek_key);
                info!(parent: self.node().span(), "the peer ID of {} is {}", addr, &peer_id);

                // upgrade noise state to post-handshake
                let FramedParts { codec, .. } = framed.into_parts();
                let NoiseCodec { noise, .. } = codec;
                let noise = noise.into_post_handshake();

                // reconstruct the Framed with the post-handshake noise state
                let mut framed = Framed::new(self.borrow_stream(&mut conn), NoiseCodec::new(noise));

                // <- protocol info
                let protocol_info = framed.try_next().await?.ok_or(io::ErrorKind::InvalidData)?;
                debug!(parent: self.node().span(), "received protocol params");

                // echo the protocol params back to the sender
                framed.send(protocol_info).await?;
                debug!(parent: self.node().span(), "echoed the protocol params back to the sender");

                (framed, peer_id)
            }
        };

        // deconstruct the framed (again) to preserve the noise state
        let FramedParts { codec, .. } = framed.into_parts();
        let NoiseCodec { noise, .. } = codec;

        // save the noise state
        self.noise_states.lock().insert(conn.addr(), noise);
        // register the peer's state
        self.peer_states
            .lock()
            .insert(peer_id, PeerState::default());

        Ok(conn)
    }
}

#[async_trait::async_trait]
impl Reading for Libp2pNode {
    type Message = YamuxMessage;
    type Codec = YamuxCodec;

    fn codec(&self, addr: SocketAddr, side: ConnectionSide) -> Self::Codec {
        let noise_state = self.noise_states.lock().get(&addr).cloned().unwrap();

        Self::Codec::new(
            NoiseCodec::new(noise_state),
            side,
            self.node().span().clone(),
        )
    }

    async fn process_message(&self, source: SocketAddr, msg: Self::Message) -> io::Result<()> {
        info!(parent: self.node().span(), "got a {:?}", msg);

        // reply to SYN messages with ACK
        if let Some(YamuxEvent::NewStream(stream_id)) = msg.event {
            let ack_msg = YamuxMessage::data(stream_id, vec![YamuxFlag::Ack], None);
            info!(parent: self.node().span(), "sending a {:?}", &ack_msg);
            self.send_direct_message(source, ack_msg)?;
        }
        // reply to pings with the same payload
        if msg.header.stream_id == 2 {
            let ping_msg = YamuxMessage::data(msg.header.stream_id, vec![], Some(msg.payload));
            info!(parent: self.node().span(), "sending a {:?}", &ping_msg);
            self.send_direct_message(source, ping_msg).unwrap();
        }

        Ok(())
    }
}

impl Writing for Libp2pNode {
    type Message = YamuxMessage;
    type Codec = YamuxCodec;

    fn codec(&self, addr: SocketAddr, side: ConnectionSide) -> Self::Codec {
        let noise_state = self.noise_states.lock().remove(&addr).unwrap();

        Self::Codec::new(
            NoiseCodec::new(noise_state),
            side,
            self.node().span().clone(),
        )
    }
}

async fn send_pings(node: Libp2pNode) {
    // advertise the use of the ping protocol
    let hello_ping = YamuxMessage::data(
        1,
        vec![YamuxFlag::Syn],
        Some(Bytes::from(
            &b"\x13/multistream/1.0.0\n\x11/ipfs/ping/1.0.0\n"[..],
        )),
    );
    info!(parent: node.node().span(), "sending a {:?}", &hello_ping);
    node.send_broadcast(hello_ping).unwrap();

    // run for a few ping rounds
    for _ in 0..5 {
        // TODO: send own pings here

        sleep(Duration::from_secs(PING_INTERVAL_SECS)).await;
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    common::start_logger(LevelFilter::DEBUG);

    // prepare the pea2pea node
    let pea2pea_node = Libp2pNode {
        node: Node::new(None).await.unwrap(),
        keypair: identity::Keypair::generate_ed25519(),
        noise_states: Default::default(),
        peer_states: Default::default(),
    };

    // enable the pea2pea protocols
    pea2pea_node.enable_handshake().await;
    pea2pea_node.enable_reading().await;
    pea2pea_node.enable_writing().await;

    // prepare and start a ping-enabled libp2p swarm
    // note: it's a leaner version of https://docs.rs/libp2p/latest/libp2p/fn.tokio_development_transport.html
    let swarm_keypair = identity::Keypair::generate_ed25519();
    let swarm_peer_id = PeerId::from(swarm_keypair.public());
    let transport = libp2p::tcp::TokioTcpConfig::new().nodelay(true);
    let noise_keys = libp2p::noise::Keypair::<libp2p::noise::X25519Spec>::new()
        .into_authentic(&swarm_keypair)
        .unwrap();
    let transport = transport
        .upgrade(libp2p::core::upgrade::Version::V1)
        .authenticate(libp2p::noise::NoiseConfig::xx(noise_keys).into_authenticated())
        .multiplex(libp2p::yamux::YamuxConfig::default())
        .timeout(Duration::from_secs(20))
        .boxed();
    let behaviour = ping::Behaviour::new(
        ping::Config::new()
            .with_keep_alive(true)
            .with_interval(Duration::from_secs(5))
            .with_timeout(Duration::from_secs(10)),
    );
    let mut swarm = Swarm::new(transport, behaviour, swarm_peer_id);

    swarm
        .listen_on("/ip4/127.0.0.1/tcp/30333".parse().unwrap())
        .unwrap();

    /*
        let pea2pea_addr = pea2pea_node.node.listening_addr().unwrap();
        let pea2pea_port = pea2pea_addr.port();

        swarm
            .dial(
                format!("/ip4/127.0.0.1/tcp/{}", pea2pea_port)
                    .parse::<libp2p::Multiaddr>()
                    .unwrap(),
            )
            .unwrap();
    */

    tokio::spawn(async move {
        loop {
            let event = swarm.select_next_some().await;
            debug!("libp2p: {:?}", event);
        }
    });

    // make sure the swarm is ready
    sleep(Duration::from_millis(100)).await;

    // connect the pea2pea node to the libp2p swarm
    let swarm_addr: SocketAddr = "127.0.0.1:30333".parse().unwrap();
    pea2pea_node.node().connect(swarm_addr).await.unwrap();

    // start sending pings
    send_pings(pea2pea_node.clone()).await;

    // send a termination message
    let terminate_msg = YamuxMessage::terminate(0); // 0 is the yamux session ID
    info!(parent: pea2pea_node.node().span(), "sending a {:?}", &terminate_msg);
    pea2pea_node
        .send_direct_message(swarm_addr, terminate_msg)
        .unwrap()
        .await
        .unwrap()
        .unwrap();

    // disconnect on pea2pea side
    pea2pea_node.node().disconnect(swarm_addr).await;

    // allow the final messages to be exchanged
    sleep(Duration::from_millis(100)).await;
}
