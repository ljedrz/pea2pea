use bytes::Bytes;
use tokio::sync::mpsc;
use tracing::*;

mod common;
use pea2pea::{
    connections::ConnectionSide,
    protocols::{Handshaking, Messaging},
    Node, NodeConfig, Pea2Pea,
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
    node: Arc<Node>,
    handshakes: Arc<RwLock<HashMap<SocketAddr, NoncePair>>>,
}

impl Pea2Pea for SecureishNode {
    fn node(&self) -> &Arc<Node> {
        &self.node
    }
}

macro_rules! read_handshake_message {
    ($expected: path, $node: expr, $conn_reader: expr, $addr: expr) => {
        if let Ok(bytes) = $conn_reader.read_exact(9).await {
            let msg: io::Result<HandshakeMsg> = if let Ok(msg) = HandshakeMsg::deserialize(bytes) {
                Ok(msg)
            } else {
                error!(parent: $node.span(), "unrecognized handshake message (neither A nor B)");
                Err(ErrorKind::Other.into())
            };

            if let Ok($expected(nonce)) = msg {
                debug!(parent: $node.span(), "received handshake message B from {}", $addr);
                Ok(nonce)
            } else {
                error!(parent: $node.span(), "received an invalid handshake message from {} (expected B)", $addr);
                Err(ErrorKind::Other.into())
            }
        } else {
            error!(parent: $node.span(), "couldn't read handshake message B");
            Err(ErrorKind::Other.into())
        }
    }
}

macro_rules! send_handshake_message {
    ($msg: expr, $node: expr, $connection: expr, $addr: expr) => {
        $connection
            .send_message($msg.serialize())
            .await;

        debug!(parent: $node.span(), "sent handshake message A to {}", $addr);
    }
}

impl_messaging!(SecureishNode);

impl Handshaking for SecureishNode {
    fn enable_handshaking(&self) {
        let (from_node_sender, mut from_node_receiver) = mpsc::channel(1);
        self.node().set_handshake_handler(from_node_sender.into());

        // spawn a background task dedicated to handling the handshakes
        let self_clone = self.clone();
        tokio::spawn(async move {
            loop {
                if let Some((mut conn_reader, conn, result_sender)) =
                    from_node_receiver.recv().await
                {
                    let node = Arc::clone(&conn_reader.node);
                    let addr = conn_reader.addr;

                    let nonce_pair = match conn.side {
                        // the connection is the Responder, so the node is the Initiator
                        ConnectionSide::Responder => {
                            debug!(parent: node.span(), "handshaking with {} as the initiator", addr);

                            // send A
                            let own_nonce = 0;
                            send_handshake_message!(HandshakeMsg::A(own_nonce), node, conn, addr);

                            // read B
                            let peer_nonce =
                                read_handshake_message!(HandshakeMsg::B, node, conn_reader, addr);

                            match peer_nonce {
                                Ok(peer_nonce) => Ok(NoncePair(own_nonce, peer_nonce)),
                                Err(e) => Err(e),
                            }
                        }
                        // the connection is the Initiator, so the node is the Responder
                        ConnectionSide::Initiator => {
                            debug!(parent: node.span(), "handshaking with {} as the responder", addr);

                            // read A
                            let peer_nonce =
                                read_handshake_message!(HandshakeMsg::A, node, conn_reader, addr);

                            // send B
                            let own_nonce = 1;
                            send_handshake_message!(HandshakeMsg::B(own_nonce), node, conn, addr);

                            match peer_nonce {
                                Ok(peer_nonce) => Ok(NoncePair(own_nonce, peer_nonce)),
                                Err(e) => Err(e),
                            }
                        }
                    };

                    let nonce_pair = match nonce_pair {
                        Ok(pair) => pair,
                        Err(e) => {
                            if result_sender.send(Err(e)).is_err() {
                                // can't recover if this happens
                                unreachable!();
                            }
                            return;
                        }
                    };

                    // register the handshake nonce
                    self_clone.handshakes.write().insert(addr, nonce_pair);

                    // return the connection objects to the node
                    if result_sender.send(Ok((conn_reader, conn))).is_err() {
                        // can't recover if this happens
                        unreachable!();
                    }
                }
            }
        });
    }
}

#[tokio::test]
async fn handshake_example() {
    tracing_subscriber::fmt::init();

    let mut initiator_config = NodeConfig::default();
    initiator_config.name = Some("initiator".into());
    let initiator = Node::new(Some(initiator_config)).await.unwrap();
    let initiator = SecureishNode {
        node: initiator,
        handshakes: Default::default(),
    };

    let mut responder_config = NodeConfig::default();
    responder_config.name = Some("responder".into());
    let responder = Node::new(Some(responder_config)).await.unwrap();
    let responder = SecureishNode {
        node: responder,
        handshakes: Default::default(),
    };

    // not required for the handshake; it's enabled only so that its relationship with the
    // handshake protocol can be tested too; it should kick in only after the handshake
    initiator.enable_messaging();
    responder.enable_messaging();

    initiator.enable_handshaking();
    responder.enable_handshaking();

    initiator
        .node
        .initiate_connection(responder.node().listening_addr)
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
    let mut initiator_config = NodeConfig::default();
    initiator_config.name = Some("initiator".into());
    let initiator = Node::new(Some(initiator_config)).await.unwrap();
    let initiator = SecureishNode {
        node: initiator,
        handshakes: Default::default(),
    };

    let mut responder_config = NodeConfig::default();
    responder_config.name = Some("responder".into());
    let responder = Node::new(Some(responder_config)).await.unwrap();
    let responder = SecureishNode {
        node: responder,
        handshakes: Default::default(),
    };

    initiator.enable_messaging();
    responder.enable_messaging();

    // the initiator doesn't enable handshaking
    responder.enable_handshaking();

    initiator
        .node
        .initiate_connection(responder.node().listening_addr)
        .await
        .unwrap();

    let message = common::prefix_with_len(2, b"this won't get through, as there was no handshake");

    initiator
        .node()
        .send_direct_message(responder.node().listening_addr, message)
        .await
        .unwrap();

    wait_until!(1, responder.node().num_connected() == 0);
}
