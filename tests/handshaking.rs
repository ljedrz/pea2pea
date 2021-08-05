use bytes::Bytes;
use parking_lot::RwLock;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tracing::*;

mod common;
use pea2pea::{
    protocols::{Handshaking, Reading, Writing},
    Connection, ConnectionSide, Node, NodeConfig, Pea2Pea,
};

use std::{collections::HashMap, convert::TryInto, io, net::SocketAddr, sync::Arc};

#[derive(Debug)]
enum HandshakeMsg {
    A(u64),
    B(u64),
}

impl HandshakeMsg {
    fn deserialize(bytes: &[u8]) -> io::Result<Self> {
        let value = u64::from_le_bytes(bytes[1..9].try_into().unwrap());

        match bytes[0] {
            0 => Ok(HandshakeMsg::A(value)),
            1 => Ok(HandshakeMsg::B(value)),
            _ => Err(io::ErrorKind::Other.into()),
        }
    }

    fn serialize(&self) -> Bytes {
        let mut ret = Vec::with_capacity(9);

        match self {
            HandshakeMsg::A(x) => {
                ret.push(0);
                ret.extend_from_slice(&x.to_le_bytes());
            }
            HandshakeMsg::B(x) => {
                ret.push(1);
                ret.extend_from_slice(&x.to_le_bytes())
            }
        }

        ret.into()
    }
}

#[derive(PartialEq, Eq)]
struct NoncePair(u64, u64); // (mine, peer's)

#[derive(Clone)]
struct SecureishNode {
    node: Node,
    handshakes: Arc<RwLock<HashMap<SocketAddr, NoncePair>>>,
}

impl Pea2Pea for SecureishNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

macro_rules! read_handshake_message {
    ($expected: path, $conn: expr) => {{
        let mut buf = [0u8; 9];

        $conn.reader().read_exact(&mut buf).await?;
        let msg = HandshakeMsg::deserialize(&buf)?;

        if let $expected(nonce) = msg {
            debug!(parent: $conn.node.span(), "received {:?} from {}", msg, $conn.addr);
            nonce
        } else {
            error!(
                parent: $conn.node.span(),
                "received an invalid handshake message from {} (expected {}, got {:?})",
                $conn.addr, stringify!($expected), msg,
            );
            return Err(io::ErrorKind::Other.into());
        }
    }}
}

macro_rules! send_handshake_message {
    ($msg: expr, $conn: expr) => {
        $conn.writer()
            .write_all(&$msg.serialize())
            .await?;

        debug!(parent: $conn.node.span(), "sent {:?} to {}", $msg, $conn.addr);
    }
}

impl_messaging!(SecureishNode);

#[async_trait::async_trait]
impl Handshaking for SecureishNode {
    async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
        let nonce_pair = match !conn.side {
            ConnectionSide::Initiator => {
                // send A
                let own_nonce = 0;
                send_handshake_message!(HandshakeMsg::A(own_nonce), conn);

                // read B
                let peer_nonce = read_handshake_message!(HandshakeMsg::B, conn);

                NoncePair(own_nonce, peer_nonce)
            }
            ConnectionSide::Responder => {
                // read A
                let peer_nonce = read_handshake_message!(HandshakeMsg::A, conn);

                // send B
                let own_nonce = 1;
                send_handshake_message!(HandshakeMsg::B(own_nonce), conn);

                NoncePair(own_nonce, peer_nonce)
            }
        };

        // register the handshake nonce
        self.handshakes.write().insert(conn.addr, nonce_pair);

        Ok(conn)
    }
}

#[tokio::test]
async fn handshake_example() {
    // tracing_subscriber::fmt::init();

    let initiator_config = NodeConfig {
        name: Some("initiator".into()),
        ..Default::default()
    };
    let initiator = Node::new(Some(initiator_config)).await.unwrap();
    let initiator = SecureishNode {
        node: initiator,
        handshakes: Default::default(),
    };

    let responder_config = NodeConfig {
        name: Some("responder".into()),
        ..Default::default()
    };
    let responder = Node::new(Some(responder_config)).await.unwrap();
    let responder = SecureishNode {
        node: responder,
        handshakes: Default::default(),
    };

    // Reading and Writing are not required for the handshake; they are enabled only so that their relationship
    // with the handshaking protocol can be tested too; they should kick in only after the handshake concludes
    for node in &[&initiator, &responder] {
        node.enable_reading();
        node.enable_writing();
        node.enable_handshaking();
    }

    initiator
        .node()
        .connect(responder.node().listening_addr().unwrap())
        .await
        .unwrap();

    wait_until!(
        1,
        initiator.handshakes.read().values().next() == Some(&NoncePair(0, 1))
            && responder.handshakes.read().values().next() == Some(&NoncePair(1, 0))
    );
}

#[tokio::test]
async fn no_handshake_no_messaging() {
    let initiator_config = NodeConfig {
        name: Some("initiator".into()),
        ..Default::default()
    };
    let initiator = Node::new(Some(initiator_config)).await.unwrap();
    let initiator = SecureishNode {
        node: initiator,
        handshakes: Default::default(),
    };

    let responder_config = NodeConfig {
        name: Some("responder".into()),
        ..Default::default()
    };
    let responder = Node::new(Some(responder_config)).await.unwrap();
    let responder = SecureishNode {
        node: responder,
        handshakes: Default::default(),
    };

    initiator.enable_writing();
    responder.enable_reading();

    // the initiator doesn't enable handshaking
    responder.enable_handshaking();

    initiator
        .node()
        .connect(responder.node().listening_addr().unwrap())
        .await
        .unwrap();

    let message = common::prefix_with_len(2, b"this won't get through, as there was no handshake");

    initiator
        .node()
        .send_direct_message(responder.node().listening_addr().unwrap(), message)
        .unwrap();

    wait_until!(1, responder.node().num_connected() == 0);
}

#[tokio::test]
async fn node_hung_handshake_fails() {
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
    impl Handshaking for Wrap {
        async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
            let _ = conn.reader().read_exact(&mut [0u8; 1]).await;

            unreachable!();
        }
    }

    let config = NodeConfig {
        max_handshake_time_ms: 10,
        ..Default::default()
    };
    let connector = Wrap(Node::new(None).await.unwrap());
    let connectee = Wrap(Node::new(Some(config)).await.unwrap());

    // note: the connector does NOT enable handshaking
    connectee.enable_handshaking();

    // the connection attempt should register just fine for the connector, as it doesn't expect a handshake
    assert!(connector
        .node()
        .connect(connectee.node().listening_addr().unwrap())
        .await
        .is_ok());

    // the TPC connection itself has been established, and with no reading, the connector doesn't know
    // that the connectee has already disconnected from it by now
    assert!(connector.node().num_connected() == 1);
    assert!(connector.node().num_connecting() == 0);

    // the connectee should have rejected the connection attempt on its side
    assert!(connectee.node().num_connected() == 0);
    assert!(connectee.node().num_connecting() == 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn node_common_timeout_when_spammed_with_connections() {
    const NUM_ATTEMPTS: u16 = 200;
    const TIMEOUT_SECS: u64 = 1;

    #[derive(Clone)]
    struct Wrap(Node);

    impl Pea2Pea for Wrap {
        fn node(&self) -> &Node {
            &self.0
        }
    }

    #[async_trait::async_trait]
    impl Handshaking for Wrap {
        async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
            conn.reader().read_exact(&mut [0u8; 1]).await?;

            Ok(conn)
        }
    }

    let config = NodeConfig {
        max_handshake_time_ms: TIMEOUT_SECS * 1_000,
        max_connections: NUM_ATTEMPTS,
        ..Default::default()
    };
    let victim = Wrap(Node::new(Some(config)).await.unwrap());
    victim.enable_handshaking();
    let victim_addr = victim.node().listening_addr().unwrap();

    let mut sockets = Vec::with_capacity(NUM_ATTEMPTS as usize);

    for _ in 0..NUM_ATTEMPTS {
        if let Ok(socket) = TcpStream::connect(victim_addr).await {
            sockets.push(socket);
        }
    }

    wait_until!(3, victim.node().num_connecting() == NUM_ATTEMPTS as usize);

    wait_until!(TIMEOUT_SECS + 1, victim.node().num_connecting() == 0);
}
