use bytes::Bytes;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
};
use tracing::*;

mod common;
use pea2pea::{
    protocols::{Handshaking, Reading, ReturnableConnection, Writing},
    ConnectionSide, Node, NodeConfig, Pea2Pea,
};

use parking_lot::RwLock;
use std::{
    collections::HashMap,
    convert::TryInto,
    io::{self, ErrorKind},
    net::SocketAddr,
    sync::Arc,
};

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
            _ => Err(ErrorKind::Other.into()),
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

        if let Ok(_) = $conn.reader().read_exact(&mut buf).await {
            let msg: io::Result<HandshakeMsg> = if let Ok(msg) = HandshakeMsg::deserialize(&buf) {
                Ok(msg)
            } else {
                error!(parent: $conn.node.span(), "unrecognized handshake message (neither A nor B)");
                Err(ErrorKind::Other.into())
            };

            if let Ok($expected(nonce)) = msg {
                debug!(parent: $conn.node.span(), "received handshake message B from {}", $conn.addr);
                Ok(nonce)
            } else {
                error!(parent: $conn.node.span(), "received an invalid handshake message from {} (expected B)", $conn.addr);
                Err(ErrorKind::Other.into())
            }
        } else {
            error!(parent: $conn.node.span(), "couldn't read handshake message B");
            Err(ErrorKind::Other.into())
        }
    }}
}

macro_rules! send_handshake_message {
    ($msg: expr, $conn: expr) => {
        $conn.writer()
            .write_all(&$msg.serialize())
            .await
            .unwrap();

        debug!(parent: $conn.node.span(), "sent handshake message A to {}", $conn.addr);
    }
}

impl_messaging!(SecureishNode);

impl Handshaking for SecureishNode {
    fn enable_handshaking(&self) {
        let (from_node_sender, mut from_node_receiver) = mpsc::channel::<ReturnableConnection>(
            self.node().config().protocol_handler_queue_depth,
        );

        // spawn a background task dedicated to handling the handshakes
        let self_clone = self.clone();
        let handshaking_task = tokio::spawn(async move {
            loop {
                if let Some((mut conn, result_sender)) = from_node_receiver.recv().await {
                    let nonce_pair = match !conn.side {
                        ConnectionSide::Initiator => {
                            debug!(parent: conn.node.span(), "handshaking with {} as the initiator", conn.addr);

                            // send A
                            let own_nonce = 0;
                            send_handshake_message!(HandshakeMsg::A(own_nonce), conn);

                            // read B
                            let peer_nonce = read_handshake_message!(HandshakeMsg::B, conn);

                            peer_nonce.map(|peer_nonce| NoncePair(own_nonce, peer_nonce))
                        }
                        ConnectionSide::Responder => {
                            debug!(parent: conn.node.span(), "handshaking with {} as the responder", conn.addr);

                            // read A
                            let peer_nonce = read_handshake_message!(HandshakeMsg::A, conn);

                            // send B
                            let own_nonce = 1;
                            send_handshake_message!(HandshakeMsg::B(own_nonce), conn);

                            peer_nonce.map(|peer_nonce| NoncePair(own_nonce, peer_nonce))
                        }
                    };

                    let nonce_pair = match nonce_pair {
                        Ok(pair) => pair,
                        Err(e) => {
                            if result_sender.send(Err(e)).is_err() {
                                unreachable!(); // can't recover if this happens
                            }
                            return;
                        }
                    };

                    // register the handshake nonce
                    self_clone.handshakes.write().insert(conn.addr, nonce_pair);

                    // return the Connection to the node
                    if result_sender.send(Ok(conn)).is_err() {
                        unreachable!(); // can't recover if this happens
                    }
                }
            }
        });

        self.node()
            .set_handshake_handler((from_node_sender, handshaking_task).into());
    }
}

#[tokio::test]
async fn handshake_example() {
    tracing_subscriber::fmt::init();

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
        .connect(responder.node().listening_addr())
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
        .connect(responder.node().listening_addr())
        .await
        .unwrap();

    let message = common::prefix_with_len(2, b"this won't get through, as there was no handshake");

    initiator
        .node()
        .send_direct_message(responder.node().listening_addr(), message)
        .await
        .unwrap();

    wait_until!(1, responder.node().num_connected() == 0);
}
