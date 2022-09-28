use deadline::deadline;
use parking_lot::RwLock;
use rand::{rngs::SmallRng, Rng, SeedableRng};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tracing::*;

mod common;
use std::{collections::HashMap, io, net::SocketAddr, sync::Arc, time::Duration};

use pea2pea::{
    protocols::{Handshake, Reading, Writing},
    Config, Connection, ConnectionSide, Node, Pea2Pea,
};

use crate::common::WritingExt;

type Nonce = u64;

#[derive(Clone)]
struct HandshakingNode {
    node: Node,
    own_nonce: Nonce,
    peer_nonces: Arc<RwLock<HashMap<Nonce, SocketAddr>>>,
}

impl HandshakingNode {
    async fn new() -> Self {
        Self {
            node: Node::new(Default::default()).await.unwrap(),
            own_nonce: SmallRng::from_entropy().gen(),
            peer_nonces: Default::default(),
        }
    }

    fn is_nonce_unique(&self, nonce: Nonce) -> bool {
        self.own_nonce != nonce && !self.peer_nonces.read().contains_key(&nonce)
    }
}

impl Pea2Pea for HandshakingNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

#[async_trait::async_trait]
impl Handshake for HandshakingNode {
    async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
        let node_conn_side = !conn.side();
        let stream = self.borrow_stream(&mut conn);

        let peer_nonce = match node_conn_side {
            ConnectionSide::Initiator => {
                // send own nonce
                stream.write_u64(self.own_nonce).await.unwrap();

                // read peer nonce
                let peer_nonce = stream.read_u64().await.unwrap();

                // check nonce uniqueness
                if !self.is_nonce_unique(peer_nonce) {
                    return Err(io::ErrorKind::AlreadyExists.into());
                }

                peer_nonce
            }
            ConnectionSide::Responder => {
                // read peer nonce
                let peer_nonce = stream.read_u64().await.unwrap();

                // check nonce uniqueness
                if !self.is_nonce_unique(peer_nonce) {
                    return Err(io::ErrorKind::AlreadyExists.into());
                }

                // send own nonce
                stream.write_u64(self.own_nonce).await.unwrap();

                peer_nonce
            }
        };

        // register the handshake nonce
        self.peer_nonces.write().insert(peer_nonce, conn.addr());

        Ok(conn)
    }
}

crate::impl_messaging!(HandshakingNode);

#[tokio::test]
async fn handshake_example() {
    let initiator = HandshakingNode::new().await;
    let responder = HandshakingNode::new().await;

    // Reading and Writing are not required for the handshake; they are enabled only so that their relationship
    // with the handshake protocol can be tested too; they should kick in only after the handshake concludes
    for node in &[&initiator, &responder] {
        node.enable_reading().await;
        node.enable_writing().await;
        node.enable_handshake().await;
    }

    initiator
        .node()
        .connect(responder.node().listening_addr().unwrap())
        .await
        .unwrap();

    deadline!(Duration::from_secs(1), move || initiator
        .peer_nonces
        .read()
        .keys()
        .next()
        == Some(&responder.own_nonce)
        && responder.peer_nonces.read().keys().next()
            == Some(&initiator.own_nonce));
}

#[tokio::test]
async fn no_handshake_no_messaging() {
    let initiator = HandshakingNode::new().await;
    let responder = HandshakingNode::new().await;

    initiator.enable_writing().await;
    responder.enable_reading().await;

    // the initiator doesn't enable handshakes
    responder.enable_handshake().await;

    initiator
        .node()
        .connect(responder.node().listening_addr().unwrap())
        .await
        .unwrap();

    let message = b"this won't get through, as there was no handshake"
        .to_vec()
        .into();

    initiator
        .send_dm(responder.node().listening_addr().unwrap(), message)
        .await
        .unwrap();

    let responder_clone = responder.clone();
    deadline!(Duration::from_secs(1), move || responder_clone
        .node()
        .num_connected()
        == 0);
    assert_eq!(responder.node().stats().received(), (0, 0));
}

// a wrapper struct with a badly implemented Handshake protocol
#[derive(Clone)]
struct Wrap(Node);

impl Pea2Pea for Wrap {
    fn node(&self) -> &Node {
        &self.0
    }
}

// a badly implemented handshake protocol; 1B is expected by both the initiator and the responder (no distinction
// is even made), but it is never provided by either of them
#[async_trait::async_trait]
impl Handshake for Wrap {
    const TIMEOUT_MS: u64 = 100;

    async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
        let _ = self
            .borrow_stream(&mut conn)
            .read_exact(&mut [0u8; 1])
            .await;

        unreachable!();
    }
}

#[tokio::test]
async fn hung_handshake_fails() {
    let connector = Wrap(Node::new(Default::default()).await.unwrap());
    let connectee = Wrap(Node::new(Default::default()).await.unwrap());

    // note: the connector does NOT enable handshakes
    connectee.enable_handshake().await;

    // the connection attempt should register just fine for the connector, as it doesn't expect a handshake
    assert!(connector
        .node()
        .connect(connectee.node().listening_addr().unwrap())
        .await
        .is_ok());

    // the TCP connection itself has been established, and with no reading, the connector doesn't know
    // that the connectee has already disconnected from it by now
    assert!(connector.node().num_connected() == 1);
    assert!(connector.node().num_connecting() == 0);

    // the connectee should have rejected the connection attempt on its side
    assert!(connectee.node().num_connected() == 0);
    assert!(connectee.node().num_connecting() == 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn timeout_when_spammed_with_connections() {
    const NUM_ATTEMPTS: u16 = 100;

    let config = Config {
        max_connections: NUM_ATTEMPTS,
        ..Default::default()
    };
    let victim = Wrap(Node::new(config).await.unwrap());
    victim.enable_handshake().await;
    let victim_addr = victim.node().listening_addr().unwrap();

    let mut sockets = Vec::with_capacity(NUM_ATTEMPTS as usize);

    for _ in 0..NUM_ATTEMPTS {
        if let Ok(socket) = TcpStream::connect(victim_addr).await {
            sockets.push(socket);
        }
    }

    let victim_clone = victim.clone();
    deadline!(Duration::from_secs(1), move || victim_clone
        .node()
        .num_connecting()
        == NUM_ATTEMPTS as usize);

    deadline!(Duration::from_secs(1), move || victim
        .node()
        .num_connecting()
        + victim.node().num_connected()
        == 0);
}
