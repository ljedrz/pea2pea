//! A noise XXpsk3 handshake example.

use std::{collections::HashMap, io, net::SocketAddr, str, sync::Arc, time::Duration};

use bytes::{Bytes, BytesMut};
use examples::noise;
use parking_lot::RwLock;
use pea2pea::{
    Config, Connection, ConnectionSide, Node, Pea2Pea,
    protocols::{Handshake, Reading, Writing},
};
use tokio::time::sleep;
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

#[derive(Clone)]
struct SecureNode {
    node: Node,
    noise_states: Arc<RwLock<HashMap<SocketAddr, noise::State>>>,
}

impl Pea2Pea for SecureNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl SecureNode {
    // create a SecureNode
    fn new(name: &str) -> Self {
        let config = Config {
            name: Some(name.into()),
            ..Default::default()
        };
        let node = Node::new(config);

        Self {
            node,
            noise_states: Default::default(),
        }
    }
}

impl Handshake for SecureNode {
    async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
        // create the noise objects
        let noise_builder =
            snow::Builder::new("Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s".parse().unwrap());
        let noise_keypair = noise_builder.generate_keypair().unwrap();
        let noise_builder = noise_builder
            .local_private_key(&noise_keypair.private)
            .unwrap();
        let noise_builder = noise_builder
            .psk(3, b"I dont care for codes of conduct")
            .unwrap();

        // perform the noise handshake
        let (noise_state, _) =
            noise::handshake_xx(self, &mut conn, noise_builder, Bytes::new()).await?;

        // save the noise state to be reused by Reading and Writing
        self.noise_states.write().insert(conn.addr(), noise_state);

        Ok(conn)
    }
}

impl Reading for SecureNode {
    type Message = BytesMut;
    type Codec = noise::Codec;

    fn codec(&self, addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        // Reading's codec is created first, so it clones the noise state; the
        // Writing codec (created last) removes it and takes final ownership
        let state = self.noise_states.read().get(&addr).cloned().unwrap();
        noise::Codec::standard(state, self.node().span().clone())
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) {
        info!(parent: self.node().span(), "decrypted a message from {source}: {message:?}");
    }
}

impl Writing for SecureNode {
    type Message = Bytes;
    type Codec = noise::Codec;

    fn codec(&self, addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        // created after Reading's codec (which cloned the state), so it can
        // remove the entry and take final ownership
        let state = self.noise_states.write().remove(&addr).unwrap();
        noise::Codec::standard(state, self.node().span().clone())
    }
}

#[tokio::main]
async fn main() {
    // DEBUG exposes the individual handshake steps
    examples::start_logger(LevelFilter::DEBUG);

    let initiator = SecureNode::new("initiator");
    let responder = SecureNode::new("responder");

    for node in &[&initiator, &responder] {
        node.enable_handshake().await; // enable the pre-defined handshakes
        node.enable_reading().await; // enable the reading protocol
        node.enable_writing().await; // enable the writing protocol
    }

    let responder_addr = responder.node().toggle_listener().await.unwrap().unwrap();

    // connect the initiator to the responder
    initiator.node().connect(responder_addr).await.unwrap();

    // determine the initiator's (ephemeral) address first
    let initiator_addr = examples::await_connection(responder.node()).await;

    // send multiple messages to double-check nonce handling
    for _ in 0..3 {
        // send a message from initiator to responder
        let msg = b"why hello there, fellow noise protocol user; I'm the initiator";
        let _ = initiator
            .unicast(responder_addr, Bytes::from(&msg[..]))
            .unwrap()
            .await;

        // send a message from responder to initiator
        let msg = b"why hello there, fellow noise protocol user; I'm the responder";
        let _ = responder
            .unicast(initiator_addr, Bytes::from(&msg[..]))
            .unwrap()
            .await;
    }

    // a small delay to ensure all messages were processed
    sleep(Duration::from_millis(10)).await;

    // nodes are never dropped implicitly; always shut them down once done
    for node in [initiator, responder] {
        node.node().shut_down().await;
    }
}
