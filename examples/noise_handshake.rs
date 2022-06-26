//! A noise XXpsk3 handshake example.

mod common;

use common::noise::{self, NoiseCodec, NoiseState};

use bytes::Bytes;
use parking_lot::RwLock;
use tokio::time::sleep;
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

use pea2pea::{
    protocols::{Handshake, Reading, Writing},
    Config, Connection, ConnectionSide, Node, Pea2Pea,
};

use std::{collections::HashMap, io, net::SocketAddr, str, sync::Arc, time::Duration};

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
        // the noise handshake pattern
        const HANDSHAKE_PATTERN: &str = "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s";

        // create the noise objects
        let noise_builder = snow::Builder::new(HANDSHAKE_PATTERN.parse().unwrap());
        let noise_keypair = noise_builder.generate_keypair().unwrap();
        let noise_builder = noise_builder.local_private_key(&noise_keypair.private);
        let noise_builder = noise_builder.psk(3, b"I dont care for codes of conduct");

        // perform the noise handshake
        let (noise_state, _) =
            noise::handshake_xx(self, &mut conn, noise_builder, Bytes::new()).await?;

        // save the noise state to be reused by Reading and Writing
        self.noise_states.write().insert(conn.addr(), noise_state);

        Ok(conn)
    }
}

#[async_trait::async_trait]
impl Reading for SecureNode {
    type Message = Bytes;
    type Codec = NoiseCodec;

    fn codec(&self, addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        let state = self.noise_states.read().get(&addr).cloned().unwrap();
        NoiseCodec::new(state)
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()> {
        info!(parent: self.node().span(), "decrypted a message from {}: {:?}", source, message);

        Ok(())
    }
}

impl Writing for SecureNode {
    type Message = Bytes;
    type Codec = NoiseCodec;

    fn codec(&self, addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
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
        let msg = b"why hello there, fellow noise protocol user; I'm the initiator";
        let _ = initiator
            .send_direct_message(
                responder.node().listening_addr().unwrap(),
                Bytes::from(&msg[..]),
            )
            .unwrap()
            .await;

        // send a message from responder to initiator
        let msg = b"why hello there, fellow noise protocol user; I'm the responder";
        let _ = responder
            .send_direct_message(initiator_addr, Bytes::from(&msg[..]))
            .unwrap()
            .await;
    }

    // a small delay to ensure all messages were processed
    sleep(Duration::from_millis(10)).await;
}
