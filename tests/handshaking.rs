use tokio::{task::JoinHandle, time::sleep};
use tracing::*;

use pea2pea::{ConnectionReader, HandshakeClosures, HandshakeProtocol, Node, NodeConfig};

use parking_lot::RwLock;
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    ops::Deref,
    sync::Arc,
    time::Duration,
};

#[derive(Clone)]
struct SecureishNode {
    node: Arc<Node>,
    handshakes: Arc<RwLock<HashMap<SocketAddr, HashSet<u64>>>>,
}

enum HandshakeMsg {
    A(u64),
    B(u64),
}

impl Deref for SecureishNode {
    type Target = Node;

    fn deref(&self) -> &Self::Target {
        &self.node
    }
}
/*
impl HandshakeProtocol for &SecureishNode {
    fn enable_handshake_protocol(&self) {
        let initiator = |node: Arc<Node>,
                         addr: SocketAddr,
                         mut connection_reader: ConnectionReader|
         -> JoinHandle<ConnectionReader> {
            tokio::spawn(async move {
                debug!(parent: node.span(), "spawned a task to handshake with {}", addr);

                // send A
                if let Err(e) = node
                    .send_direct_message(addr, vec![HandshakeMsg::A as u8]) // TODO: send nonce
                    .await
                {
                    error!(parent: node.span(), "can't send handshake message A to {}: {}", addr, e);
                // TODO: kick, register failure
                } else {
                    debug!(parent: node.span(), "sent handshake message A to {}", addr);
                };

                // read B
                match connection_reader.read_message().await {
                    Ok(msg) => {
                        // node.known_peers.register_incoming_message(addr, msg.len());

                        if msg[0] == HandshakeMsg::B as u8 {
                            // TODO: register nonce
                            debug!(parent: node.span(), "received handshake message B from {}", addr);
                        } else {
                            error!(parent: node.span(), "received an invalid handshake message from {}", addr);
                            // TODO: kick, register failure
                        }
                    }
                    Err(e) => {
                        // TODO: kick, register failure
                        error!(parent: node.span(), "can't read message: {}", e);
                    }
                }

                connection_reader
            })
        };

        let responder = |node: Arc<Node>,
                         addr: SocketAddr,
                         mut connection_reader: ConnectionReader|
         -> JoinHandle<ConnectionReader> {
            tokio::spawn(async move {
                debug!(parent: node.span(), "spawned a task to handshake with {}", addr);

                // read A
                match connection_reader.read_message().await {
                    Ok(msg) => {
                        // node.known_peers.register_incoming_message(addr, msg.len());

                        if msg[0] == HandshakeMsg::A as u8 {
                            // TODO: register nonce
                            debug!(parent: node.span(), "received handshake message A from {}", addr);
                        } else {
                            error!(parent: node.span(), "received an invalid handshake message from {}", addr);
                            // TODO: kick, register failure
                        }
                    }
                    Err(e) => {
                        // TODO: kick, register failure
                        error!(parent: node.span(), "can't read message: {}", e);
                    }
                }

                // send B
                if let Err(e) = node
                    .send_direct_message(addr, vec![HandshakeMsg::B as u8]) // TODO: send nonce
                    .await
                {
                    error!(parent: node.span(), "can't send handshake message B to {}: {}", addr, e);
                // TODO: kick, register failure
                } else {
                    debug!(parent: node.span(), "sent handshake message B to {}", addr);
                };

                connection_reader
            })
        };

        let handshake_closures = HandshakeClosures {
            initiator: Box::new(initiator),
            responder: Box::new(responder),
        };

        if self
            .node
            .handshake_closures
            .set(Some(handshake_closures))
            .is_err()
        {
            unreachable!()
        }
    }
}

#[tokio::test]
async fn simple_handshake() {
    tracing_subscriber::fmt::init();

    let mut initiator_node_config = NodeConfig::default();
    initiator_node_config.name = Some("initiator".into());
    let initiator_node = Node::new(Some(initiator_node_config)).await.unwrap();
    let initiator_node = Arc::new(SecureishNode {
        node: initiator_node,
        handshakes: Default::default(),
    });

    let mut responder_node_config = NodeConfig::default();
    responder_node_config.name = Some("responder".into());
    let responder_node = Node::new(Some(responder_node_config)).await.unwrap();
    let responder_node = Arc::new(SecureishNode {
        node: responder_node,
        handshakes: Default::default(),
    });

    initiator_node.enable_handshake_protocol();
    responder_node.enable_handshake_protocol();

    initiator_node
        .node
        .initiate_connection(responder_node.local_addr)
        .await
        .unwrap();

    sleep(Duration::from_millis(100)).await;

    assert!(initiator_node.is_handshaken(responder_node.local_addr));
    // TODO: the responder is also handshaken, but it knows the initiator under a different address;
    // obtain it and then perform the assertion
}
*/
