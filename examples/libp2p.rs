//! An minimal node capable of communicating with libp2p nodes.
//!
//! The supported libp2p protocol is ping.

mod common;

use std::{cmp, collections::HashMap, io, net::SocketAddr, sync::Arc, time::Duration};

use bytes::{Bytes, BytesMut};
use common::{noise, yamux};
use futures_util::{SinkExt, StreamExt, TryStreamExt};
use libp2p::swarm::{keep_alive, Swarm, SwarmEvent};
use libp2p::{core::multiaddr::Protocol, identity, ping, NetworkBehaviour, PeerId, Transport};
use parking_lot::{Mutex, RwLock};
use pea2pea::{
    protocols::{Disconnect, Handshake, Reading, Writing},
    Connection, ConnectionSide, Node, Pea2Pea,
};
use prost::Message;
use tokio::{sync::oneshot, time::sleep};
use tokio_util::codec::{Decoder, Encoder, Framed, FramedParts};
use tracing::*;
use tracing_subscriber::filter::LevelFilter;
use unsigned_varint::codec::UviBytes;

// the protocol string of libp2p::ping
const PROTOCOL_PING: &[u8] = b"\x13/multistream/1.0.0\n\x11/ipfs/ping/1.0.0\n";

// the libp2p-cabable node
#[derive(Clone)]
struct Libp2pNode {
    // the pea2pea node
    node: Node,
    // the libp2p keypair
    keypair: identity::Keypair,
    // the libp2p PeerId
    #[allow(dead_code)]
    peer_id: PeerId,
    // holds noise states between the handshake and the other protocols
    noise_states: Arc<Mutex<HashMap<SocketAddr, noise::State>>>,
    // holds the state related to peers
    peer_states: Arc<RwLock<HashMap<SocketAddr, PeerState>>>,
}

impl Pea2Pea for Libp2pNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Libp2pNode {
    async fn new() -> Self {
        let keypair = identity::Keypair::generate_ed25519();
        let peer_id = keypair.public().to_peer_id();

        let node = Node::new(Default::default()).await.unwrap();

        info!(parent: node.span(), "started a node with PeerId {}", peer_id);

        Self {
            node,
            keypair,
            peer_id,
            noise_states: Default::default(),
            peer_states: Default::default(),
        }
    }

    async fn process_event(&self, event: Event, source: SocketAddr) -> io::Result<()> {
        let reply = match event {
            Event::NewStream(stream_id, protocol_info) => {
                // reply to SYN flag with the ACK one
                Some(yamux::Frame::data(
                    stream_id,
                    vec![yamux::Flag::Ack],
                    Some(protocol_info),
                ))
            }
            Event::StreamHalfClosed(stream_id) => {
                // reply with own half-close
                Some(yamux::Frame::data(stream_id, vec![yamux::Flag::Fin], None))
            }
            Event::ReceivedPing(stream_id, payload) => {
                // reply to pings with the same payload
                Some(yamux::Frame::data(stream_id, vec![], Some(payload)))
            }
            _ => None,
        };

        if let Some(reply_msg) = reply {
            info!(parent: self.node().span(), " sending a {:?}", &reply_msg);
            let _ = self.unicast(source, reply_msg)?.await;
        }

        Ok(())
    }
}

// a set of yamux streams belonging to a single connection
pub type Streams = HashMap<yamux::StreamId, Bytes>;

// the state applicable to a single peer
struct PeerState {
    #[allow(dead_code)]
    id: PeerId,
    streams: Streams,
}

// the event indicated by the contents of a yamux message
#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    NewStream(yamux::StreamId, Bytes),
    StreamHalfClosed(yamux::StreamId),
    StreamTerminated(yamux::StreamId),
    ReceivedPing(yamux::StreamId, Bytes),
    Unknown(yamux::Frame),
}

// a codec capable of (de/en)coding libp2p messages with noise and yamux protocols
struct Codec {
    noise: noise::Codec,
    yamux: yamux::Codec,
}

impl Codec {
    fn new(noise: noise::Codec, yamux: yamux::Codec) -> Self {
        Self { noise, yamux }
    }
}

impl Decoder for Codec {
    type Item = yamux::Frame;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // decrypt raw bytes using noise
        let mut bytes = if let Some(bytes) = self.noise.decode(src)? {
            bytes
        } else {
            return Ok(None);
        };

        // decode the Yamux frame
        self.yamux.decode(&mut bytes)
    }
}

impl Encoder<yamux::Frame> for Codec {
    type Error = io::Error;

    fn encode(&mut self, msg: yamux::Frame, dst: &mut BytesMut) -> Result<(), Self::Error> {
        // encode the Yamux frame
        self.yamux.encode(msg, dst)?;
        let mut bytes = dst.split().freeze();

        // split the frame into noise-compatible chunks
        while !bytes.is_empty() {
            let chunk = bytes.split_to(cmp::min(bytes.len(), noise::MAX_MESSAGE_LEN));
            self.noise.encode(chunk, dst)?;
        }

        Ok(())
    }
}

// payloads for Noise handshake messages
// note: this struct was auto-generated using prost-build based on
// the proto file from the libp2p-noise repository
#[derive(Clone, PartialEq, Eq, ::prost::Message)]
pub struct NoiseHandshakePayload {
    #[prost(bytes = "vec", tag = "1")]
    pub identity_key: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub identity_sig: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    pub data: ::prost::alloc::vec::Vec<u8>,
}

#[async_trait::async_trait]
impl Handshake for Libp2pNode {
    const TIMEOUT_MS: u64 = 5_000;

    async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
        let node_conn_side = !conn.side();
        let addr = conn.addr();

        // TODO: try to use it post-handshake too when parsing protocol info
        let mut negotiation_codec = Framed::new(self.borrow_stream(&mut conn), UviBytes::default());

        // exchange libp2p protocol params
        match node_conn_side {
            ConnectionSide::Initiator => {
                // -> protocol info (1/2)
                negotiation_codec
                    .send(Bytes::from("/multistream/1.0.0\n"))
                    .await?;
                debug!(parent: self.node().span(), "sent protocol params (1/2)");

                // <- protocol info (1/2)
                let protocol_info = negotiation_codec
                    .try_next()
                    .await?
                    .ok_or(io::ErrorKind::InvalidData)?;
                debug!(parent: self.node().span(), "received protocol params (1/2): {:?}", protocol_info);

                // -> protocol info (2/2)
                negotiation_codec.send(Bytes::from("/noise\n")).await?;
                debug!(parent: self.node().span(), "sent protocol params (2/2)");

                // <- protocol info (2/2)
                let protocol_info = negotiation_codec
                    .try_next()
                    .await?
                    .ok_or(io::ErrorKind::InvalidData)?;
                debug!(parent: self.node().span(), "received protocol params (2/2): {:?}", protocol_info);
            }
            ConnectionSide::Responder => {
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
            }
        };

        // create the noise objects
        let noise_builder = snow::Builder::new("Noise_XX_25519_ChaChaPoly_SHA256".parse().unwrap());
        let noise_keypair = noise_builder.generate_keypair().unwrap();
        let noise_builder = noise_builder.local_private_key(&noise_keypair.private);

        // prepare the expected handshake payload
        let noise_payload = {
            let protobuf_payload = NoiseHandshakePayload {
                identity_key: self.keypair.public().to_protobuf_encoding(),
                identity_sig: self
                    .keypair
                    .sign(&[&b"noise-libp2p-static-key:"[..], &noise_keypair.public].concat())
                    .unwrap(),
                data: vec![],
            };
            let mut bytes = Vec::with_capacity(protobuf_payload.encoded_len());
            protobuf_payload.encode(&mut bytes).unwrap();
            bytes
        };

        // perform the noise handshake
        let (noise_state, secure_payload) =
            noise::handshake_xx(self, &mut conn, noise_builder, noise_payload.into()).await?;

        // obtain the PeerId from the handshake payload
        let secure_payload = NoiseHandshakePayload::decode(&secure_payload[..])?;
        let peer_key = identity::PublicKey::from_protobuf_encoding(&secure_payload.identity_key)
            .map_err(|_| io::ErrorKind::InvalidData)?;
        let peer_id = PeerId::from(peer_key);
        info!(parent: self.node().span(), "the PeerId of {} is {}", addr, &peer_id);

        // reconstruct the Framed with the post-handshake noise state
        let mut framed = Framed::new(
            self.borrow_stream(&mut conn),
            noise::Codec::new(
                2,
                u16::MAX as usize,
                noise_state,
                self.node().span().clone(),
            ),
        );

        // exchange further protocol params
        match node_conn_side {
            ConnectionSide::Initiator => {
                // -> protocol info (1/2)
                framed
                    .send(Bytes::from(&b"\x13/multistream/1.0.0\n"[..]))
                    .await?;
                debug!(parent: self.node().span(), "sent protocol params (1/2)");

                // <- protocol info (1/2)
                let protocol_info = framed.try_next().await?.ok_or(io::ErrorKind::InvalidData)?;
                debug!(parent: self.node().span(), "received protocol params (1/2): {:?}", protocol_info);

                // -> protocol info (2/2)
                framed.send(Bytes::from(&b"\r/yamux/1.0.0\n"[..])).await?;
                debug!(parent: self.node().span(), "sent protocol params (2/2)");

                // <- protocol info (2/2)
                let protocol_info = framed.try_next().await?.ok_or(io::ErrorKind::InvalidData)?;
                debug!(parent: self.node().span(), "received protocol params (2/2): {:?}", protocol_info);
            }
            ConnectionSide::Responder => {
                // <- protocol info
                let protocol_info = framed.try_next().await?.ok_or(io::ErrorKind::InvalidData)?;
                debug!(parent: self.node().span(), "received protocol params: {:?}", protocol_info);

                // echo the protocol params back to the sender
                framed.send(protocol_info.freeze()).await?;
                debug!(parent: self.node().span(), "echoed the protocol params back to the sender");
            }
        }

        // deconstruct the framed (again) to preserve the noise state
        let FramedParts {
            codec, read_buf, ..
        } = framed.into_parts();
        let noise::Codec { mut noise, .. } = codec;

        // preserve any unprocessed bytes
        noise.save_buffer(read_buf);

        // save the noise state
        self.noise_states.lock().insert(conn.addr(), noise);
        // register the peer's state
        self.peer_states.write().insert(
            addr,
            PeerState {
                id: peer_id,
                streams: Default::default(),
            },
        );

        Ok(conn)
    }
}

macro_rules! get_streams_mut {
    ($self:expr, $addr:expr) => {
        $self
            .peer_states
            .write()
            .get_mut(&$addr)
            .ok_or(io::ErrorKind::BrokenPipe)?
            .streams
    };
}

#[async_trait::async_trait]
impl Reading for Libp2pNode {
    type Message = yamux::Frame;
    type Codec = Codec;

    fn codec(&self, addr: SocketAddr, side: ConnectionSide) -> Self::Codec {
        let noise_state = self.noise_states.lock().get(&addr).cloned().unwrap();

        Self::Codec::new(
            noise::Codec::new(
                2,
                u16::MAX as usize,
                noise_state,
                self.node().span().clone(),
            ),
            yamux::Codec::new(side, self.node().span().clone()),
        )
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()> {
        info!(parent: self.node().span(), "received a {:?}", message);

        // deconstruct the yamux frame
        let yamux::Header {
            stream_id, flags, ..
        } = &message.header;
        let payload = message.payload;

        // register the discovered events
        let mut events = vec![];

        match &flags[..] {
            &[yamux::Flag::Syn] => {
                // a new stream is being created
                if get_streams_mut!(self, source)
                    .insert(*stream_id, payload.clone())
                    .is_none()
                {
                    events.push(Event::NewStream(*stream_id, payload.clone()));
                } else {
                    error!(parent: self.node().span(), "yamux stream {} had already been registered", stream_id);
                    return Err(io::ErrorKind::InvalidData.into());
                }
            }
            &[yamux::Flag::Rst] => {
                // a stream is being terminated
                if get_streams_mut!(self, source).remove(stream_id).is_some() {
                    events.push(Event::StreamTerminated(*stream_id));
                } else {
                    error!(parent: self.node().span(), "yamux stream {} is unknown", stream_id);
                    return Err(io::ErrorKind::InvalidData.into());
                }
            }
            &[yamux::Flag::Ack] => {
                todo!(); // TODO: only matters when starting own streams
            }
            &[yamux::Flag::Fin] => {
                // a stream is being half-closed
                if get_streams_mut!(self, source).remove(stream_id).is_some() {
                    events.push(Event::StreamHalfClosed(*stream_id));
                } else {
                    error!(parent: self.node().span(), "yamux stream {} is unknown", stream_id);
                    return Err(io::ErrorKind::InvalidData.into());
                }
            }
            &[] => {
                // a regular message in an established stream
                let protocol = if let Some(p) = self
                    .peer_states
                    .read()
                    .get(&source)
                    .ok_or(io::ErrorKind::BrokenPipe)?
                    .streams
                    .get(stream_id)
                {
                    p.clone()
                } else {
                    error!(parent: self.node().span(), "yamux stream {} is unknown", stream_id);
                    return Err(io::ErrorKind::InvalidData.into());
                };

                if protocol == PROTOCOL_PING {
                    events.push(Event::ReceivedPing(*stream_id, payload));
                } else {
                    events.push(Event::Unknown(yamux::Frame {
                        header: message.header,
                        payload,
                    }));
                }
            }
            flags => {
                warn!(parent: self.node().span(), "unexpected combination of yamux flags: {:?}", flags);
            }
        }

        for event in events {
            self.process_event(event, source).await?;
        }

        Ok(())
    }
}

impl Writing for Libp2pNode {
    type Message = yamux::Frame;
    type Codec = Codec;

    fn codec(&self, addr: SocketAddr, side: ConnectionSide) -> Self::Codec {
        let noise_state = self.noise_states.lock().remove(&addr).unwrap();

        Self::Codec::new(
            noise::Codec::new(
                2,
                u16::MAX as usize,
                noise_state,
                self.node().span().clone(),
            ),
            yamux::Codec::new(side, self.node().span().clone()),
        )
    }
}

#[async_trait::async_trait]
impl Disconnect for Libp2pNode {
    async fn handle_disconnect(&self, addr: SocketAddr) {
        self.peer_states.write().remove(&addr);
    }
}

#[derive(NetworkBehaviour, Default)]
struct Behaviour {
    keep_alive: keep_alive::Behaviour,
    ping: ping::Behaviour,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    common::start_logger(LevelFilter::DEBUG);

    // prepare the pea2pea node
    let pea2pea_node = Libp2pNode::new().await;

    // enable the pea2pea protocols
    pea2pea_node.enable_handshake().await;
    pea2pea_node.enable_reading().await;
    pea2pea_node.enable_writing().await;

    // prepare and start a ping-enabled libp2p swarm
    // note: it's a leaner version of https://docs.rs/libp2p/latest/libp2p/fn.tokio_development_transport.html
    let swarm_keypair = identity::Keypair::generate_ed25519();
    let swarm_peer_id = PeerId::from(swarm_keypair.public());
    let transport =
        libp2p::tcp::TokioTcpTransport::new(libp2p::tcp::GenTcpConfig::new().nodelay(true));
    let noise_keys = libp2p::noise::Keypair::<libp2p::noise::X25519Spec>::new()
        .into_authentic(&swarm_keypair)
        .unwrap();
    let transport = transport
        .upgrade(libp2p::core::upgrade::Version::V1)
        .authenticate(libp2p::noise::NoiseConfig::xx(noise_keys).into_authenticated())
        .multiplex(libp2p::yamux::YamuxConfig::default())
        .boxed();
    let mut swarm = Swarm::new(transport, Behaviour::default(), swarm_peer_id);

    swarm
        .listen_on("/ip4/127.0.0.1/tcp/0".parse().unwrap())
        .unwrap();

    // this channel will allow us to discover the swarm's listening port
    let (tx, rx) = oneshot::channel();

    tokio::spawn(async move {
        let mut tx = Some(tx);
        loop {
            let event = swarm.select_next_some().await;
            debug!("   libp2p node: {:?}", event);
            if let SwarmEvent::NewListenAddr { address, .. } = event {
                tx.take().unwrap().send(address).unwrap();
            }
        }
    });

    let mut swarm_addr = rx.await.unwrap();
    let swarm_port = if let Some(Protocol::Tcp(port)) = swarm_addr.pop() {
        port
    } else {
        panic!("the libp2p swarm did not return a listening TCP port");
    };

    // connect the pea2pea node to the libp2p swarm
    let swarm_addr = format!("127.0.0.1:{}", swarm_port).parse().unwrap();
    pea2pea_node.node().connect(swarm_addr).await.unwrap();

    // allow a few messages to be exchanged
    sleep(Duration::from_secs(60)).await;
}
