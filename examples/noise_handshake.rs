mod common;

use bytes::{Buf, BytesMut};
use futures_util::{sink::SinkExt, TryStreamExt};
use parking_lot::RwLock;
use tokio::time::sleep;
use tokio_util::codec::{Decoder, Encoder, Framed, FramedParts, LengthDelimitedCodec};
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

use pea2pea::{
    protocols::{Handshake, Reading, Writing},
    Config, Connection, ConnectionSide, Node, Pea2Pea,
};

use std::{collections::HashMap, io, net::SocketAddr, str, sync::Arc, time::Duration};

// maximum noise message size, as specified by its protocol
const NOISE_MAX_LEN: usize = 65535;

#[derive(Clone)]
struct SecureNode {
    node: Node,
    noise_states: Arc<RwLock<HashMap<SocketAddr, NoiseState>>>,
}

impl Pea2Pea for SecureNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

enum NoiseState {
    Handshake(Box<snow::HandshakeState>),
    PostHandshake(Arc<snow::StatelessTransportState>),
}

impl Clone for NoiseState {
    fn clone(&self) -> Self {
        Self::PostHandshake(Arc::clone(self.post_handshake()))
    }
}

impl NoiseState {
    fn handshake(&mut self) -> Option<&mut snow::HandshakeState> {
        if let Self::Handshake(state) = self {
            Some(state)
        } else {
            None
        }
    }

    fn into_post_handshake(self) -> Self {
        if let Self::Handshake(state) = self {
            let state = state.into_stateless_transport_mode().unwrap();
            Self::PostHandshake(Arc::new(state))
        } else {
            panic!();
        }
    }

    fn post_handshake(&self) -> &Arc<snow::StatelessTransportState> {
        if let Self::PostHandshake(state) = self {
            state
        } else {
            panic!();
        }
    }
}

struct NoiseCodec {
    codec: LengthDelimitedCodec,
    noise: NoiseState,
    buffer: Box<[u8]>,
    nonce: u64,
}

impl NoiseCodec {
    fn new(noise: NoiseState) -> Self {
        NoiseCodec {
            codec: LengthDelimitedCodec::builder()
                .max_frame_length(NOISE_MAX_LEN)
                .new_codec(),
            noise,
            nonce: 0,
            buffer: vec![0u8; NOISE_MAX_LEN].into(),
        }
    }
}

impl Decoder for NoiseCodec {
    type Item = String;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let bytes = if let Some(bytes) = self.codec.decode(src)? {
            bytes
        } else {
            return Ok(None);
        };

        let msg_len = if let Some(noise) = self.noise.handshake() {
            noise
                .read_message(bytes.chunk(), &mut self.buffer)
                .map_err(|_| io::ErrorKind::InvalidData)?
        } else {
            let len = self
                .noise
                .post_handshake()
                .read_message(self.nonce, bytes.chunk(), &mut self.buffer)
                .map_err(|_| io::ErrorKind::InvalidData)?;
            self.nonce += 1;
            len
        };
        let msg = String::from_utf8(self.buffer[..msg_len].to_vec()).unwrap();

        Ok(Some(msg))
    }
}

impl Encoder<String> for NoiseCodec {
    type Error = io::Error;

    fn encode(&mut self, msg: String, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let msg_len = if let Some(noise) = self.noise.handshake() {
            noise.write_message(&[], &mut self.buffer).unwrap()
        } else {
            let len = self
                .noise
                .post_handshake()
                .write_message(self.nonce, msg.as_bytes(), &mut self.buffer)
                .map_err(|_| io::ErrorKind::InvalidData)?;
            self.nonce += 1;
            len
        };
        let msg = self.buffer[..msg_len].to_vec().into();

        self.codec.encode(msg, dst)
    }
}

impl SecureNode {
    // create a SecureNode
    async fn new(name: &str) -> io::Result<Self> {
        let config = Config {
            name: Some(name.into()),
            ..Default::default()
        };
        let node = Node::new(Some(config)).await?;

        Ok(Self {
            node,
            noise_states: Default::default(),
        })
    }
}

#[async_trait::async_trait]
impl Handshake for SecureNode {
    async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
        // the noise handshake settings used by snow
        const HANDSHAKE_PATTERN: &str = "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s";
        const PRE_SHARED_KEY: &[u8] = b"I dont care for codes of conduct"; // the PSK must be 32B

        let builder = snow::Builder::new(HANDSHAKE_PATTERN.parse().unwrap());
        let static_key = builder.generate_keypair().unwrap().private;
        let noise_builder = builder
            .local_private_key(&static_key)
            .psk(3, PRE_SHARED_KEY);

        let node_conn_side = !conn.side();
        let stream = self.borrow_stream(&mut conn);

        let noise = match node_conn_side {
            ConnectionSide::Initiator => {
                let noise = Box::new(noise_builder.build_initiator().unwrap());
                let mut framed = Framed::new(stream, NoiseCodec::new(NoiseState::Handshake(noise)));

                // -> e
                framed.send("".into()).await?;
                debug!(parent: self.node().span(), "sent e (XX handshake part 1/3)");

                // <- e, ee, s, es
                framed.try_next().await?;
                debug!(parent: self.node().span(), "received e, ee, s, es (XX handshake part 2/3)");

                // -> s, se, psk
                framed.send("".into()).await?;
                debug!(parent: self.node().span(), "sent s, se, psk (XX handshake part 3/3)");

                let FramedParts { codec, .. } = framed.into_parts();
                let NoiseCodec { noise, .. } = codec;
                noise.into_post_handshake()
            }
            ConnectionSide::Responder => {
                let noise = Box::new(noise_builder.build_responder().unwrap());
                let mut framed = Framed::new(stream, NoiseCodec::new(NoiseState::Handshake(noise)));

                // <- e
                framed.try_next().await?;
                debug!(parent: self.node().span(), "received e (XX handshake part 1/3)");

                // -> e, ee, s, es
                framed.send("".into()).await?;
                debug!(parent: self.node().span(), "sent e, ee, s, es (XX handshake part 2/3)");

                // <- s, se, psk
                framed.try_next().await?;
                debug!(parent: self.node().span(), "received s, se, psk (XX handshake part 3/3)");

                let FramedParts { codec, .. } = framed.into_parts();
                let NoiseCodec { noise, .. } = codec;
                noise.into_post_handshake()
            }
        };

        debug!(parent: self.node().span(), "XX handshake complete");

        self.noise_states.write().insert(conn.addr(), noise);

        Ok(conn)
    }
}

#[async_trait::async_trait]
impl Reading for SecureNode {
    type Message = String;
    type Codec = NoiseCodec;

    fn codec(&self, addr: SocketAddr) -> Self::Codec {
        let state = self.noise_states.read().get(&addr).cloned().unwrap();
        NoiseCodec::new(state)
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()> {
        info!(parent: self.node().span(), "decrypted a message from {}: \"{}\"", source, message);

        Ok(())
    }
}

impl Writing for SecureNode {
    type Message = String;
    type Codec = NoiseCodec;

    fn codec(&self, addr: SocketAddr) -> Self::Codec {
        let state = self.noise_states.write().remove(&addr).unwrap();
        NoiseCodec::new(state)
    }
}

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::TRACE);

    let initiator = SecureNode::new("initiator").await.unwrap();
    let responder = SecureNode::new("responder").await.unwrap();

    for node in &[&initiator, &responder] {
        node.enable_handshake().await; // enable the pre-defined handshakes
        node.enable_reading().await; // enable the reading protocol
        node.enable_writing().await; // enable the writing protocol
    }

    // connect the initiator to the responder
    initiator
        .node()
        .connect(responder.node().listening_addr().unwrap())
        .await
        .unwrap();

    // determine the initiator's address first
    sleep(Duration::from_millis(10)).await;
    let initiator_addr = responder.node().connected_addrs()[0];

    // send multiple messages to double-check nonce handling
    for _ in 0..3 {
        // send a message from initiator to responder
        let msg = "why hello there, fellow noise protocol user; I'm the initiator";
        let _ = initiator
            .send_direct_message(responder.node().listening_addr().unwrap(), msg.to_string())
            .unwrap()
            .await;

        // send a message from responder to initiator
        let msg = "why hello there, fellow noise protocol user; I'm the responder";
        let _ = responder
            .send_direct_message(initiator_addr, msg.to_string())
            .unwrap()
            .await;
    }

    // a small delay to ensure all messages were processed
    sleep(Duration::from_millis(10)).await;
}
