//! An minimal node capable of communicating with libp2p nodes.
//!
//! The supported libp2p protocol is ping.

mod common;

use common::{
    noise::{self, NoiseCodec, NoiseState},
    yamux,
};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt, TryStreamExt};
use libp2p::swarm::Swarm;
use libp2p::{identity, ping, PeerId, Transport};
use parking_lot::Mutex;
use prost::Message;
use tokio::time::sleep;
use tokio_util::codec::{Decoder, Encoder, Framed, FramedParts};
use tracing::*;
use tracing_subscriber::filter::LevelFilter;
use unsigned_varint::codec::UviBytes;

use pea2pea::{
    protocols::{Disconnect, Handshake, Reading, Writing},
    Connection, ConnectionSide, Node, Pea2Pea,
};

use std::{collections::HashMap, fmt, io, net::SocketAddr, sync::Arc, time::Duration};

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

// the protocol string of libp2p::ping
const PROTOCOL_PING: &[u8] = b"\x13/multistream/1.0.0\n\x11/ipfs/ping/1.0.0\n";

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
    peer_states: Arc<Mutex<HashMap<SocketAddr, PeerState>>>,
}

impl Pea2Pea for Libp2pNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Libp2pNode {
    async fn process_event(&self, event: Event, source: SocketAddr) -> io::Result<()> {
        info!(parent: self.node().span(), "{}", event);

        let reply = match event {
            Event::NewStream(stream_id) => {
                // reply to SYN flag with the ACK one
                Some(yamux::Frame::data(stream_id, vec![yamux::Flag::Ack], None))
            }
            Event::ReceivedPing(stream_id, payload) => {
                // reply to pings with the same payload
                Some(yamux::Frame::data(stream_id, vec![], Some(payload)))
            }
            _ => None,
        };

        if let Some(reply_msg) = reply {
            info!(parent: self.node().span(), "sending a {:?}", &reply_msg);
            let _ = self.send_direct_message(source, reply_msg)?.await;
        }

        Ok(())
    }
}

#[allow(dead_code)]
struct PeerState {
    id: PeerId,
}

// the event indicated by the contents of a yamux message
#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    NewStream(yamux::StreamId),
    StreamTerminated(yamux::StreamId),
    ReceivedPing(yamux::StreamId, Bytes),
    Unknown(yamux::Frame),
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NewStream(id) => write!(f, "registered a new inbound yamux stream (id = {})", id),
            Self::StreamTerminated(id) => {
                write!(f, "received a termination message for yamux stream {}", id)
            }
            Self::ReceivedPing(id, payload) => {
                write!(
                    f,
                    "received a ping (stream = {}, payload = {:?})",
                    id, payload
                )
            }
            Self::Unknown(msg) => write!(f, "received an unknown message: {:?}", msg),
        }
    }
}

#[async_trait::async_trait]
impl Handshake for Libp2pNode {
    const TIMEOUT_MS: u64 = 5_000;

    async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
        let node_conn_side = !conn.side();
        let addr = conn.addr();
        let stream = self.borrow_stream(&mut conn);

        // exchange libp2p protocol params
        match node_conn_side {
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
                debug!(parent: self.node().span(), "received protocol params (2/2)");
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
            }
        };

        // the noise handshake pattern
        const HANDSHAKE_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_SHA256";

        // create the noise objects
        let noise_builder = snow::Builder::new(HANDSHAKE_PATTERN.parse().unwrap());
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

        // obtain the peer ID from the handshake payload
        let secure_payload = NoiseHandshakePayload::decode(&secure_payload[..])?;
        let peer_key = identity::PublicKey::from_protobuf_encoding(&secure_payload.identity_key)
            .map_err(|_| io::ErrorKind::InvalidData)?;
        let peer_id = PeerId::from(peer_key);
        info!(parent: self.node().span(), "the peer ID of {} is {}", addr, &peer_id);

        // exchange further protocol params
        let framed = match node_conn_side {
            ConnectionSide::Initiator => {
                // reconstruct the Framed with the post-handshake noise state
                let mut framed =
                    Framed::new(self.borrow_stream(&mut conn), NoiseCodec::new(noise_state));

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

                framed
            }
            ConnectionSide::Responder => {
                // reconstruct the Framed with the post-handshake noise state
                let mut framed =
                    Framed::new(self.borrow_stream(&mut conn), NoiseCodec::new(noise_state));

                // <- protocol info
                let protocol_info = framed.try_next().await?.ok_or(io::ErrorKind::InvalidData)?;
                debug!(parent: self.node().span(), "received protocol params");

                // echo the protocol params back to the sender
                framed.send(protocol_info).await?;
                debug!(parent: self.node().span(), "echoed the protocol params back to the sender");

                framed
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
            .insert(addr, PeerState { id: peer_id });

        Ok(conn)
    }
}

#[async_trait::async_trait]
impl Reading for Libp2pNode {
    type Message = Vec<Event>;
    type Codec = yamux::Codec<Bytes>;

    fn codec(&self, addr: SocketAddr, side: ConnectionSide) -> Self::Codec {
        let noise_state = self.noise_states.lock().get(&addr).cloned().unwrap();

        Self::Codec::new(
            NoiseCodec::new(noise_state),
            side,
            self.node().span().clone(),
        )
    }

    async fn process_message(&self, source: SocketAddr, mut events: Vec<Event>) -> io::Result<()> {
        while let Some(event) = events.pop() {
            self.process_event(event, source).await?;
        }

        Ok(())
    }
}

impl Writing for Libp2pNode {
    type Message = yamux::Frame;
    type Codec = yamux::Codec<Bytes>;

    fn codec(&self, addr: SocketAddr, side: ConnectionSide) -> Self::Codec {
        let noise_state = self.noise_states.lock().remove(&addr).unwrap();

        Self::Codec::new(
            NoiseCodec::new(noise_state),
            side,
            self.node().span().clone(),
        )
    }
}

#[async_trait::async_trait]
impl Disconnect for Libp2pNode {
    async fn handle_disconnect(&self, addr: SocketAddr) {
        self.peer_states.lock().remove(&addr);
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

    // allow a few messages to be exchanged
    sleep(Duration::from_secs(30)).await;

    // send a termination message
    let terminate_msg = yamux::Frame::terminate(0); // 0 is the yamux session ID
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
