use tokio::{io::AsyncReadExt, task::JoinHandle, time::sleep};
use tracing::*;

mod common;
use pea2pea::{
    Connection, ConnectionReader, ContainsNode, HandshakeClosures, HandshakeProtocol,
    MessagingProtocol, Node, NodeConfig, PacketingProtocol,
};

use parking_lot::RwLock;
use std::{
    collections::{HashMap, HashSet},
    convert::TryInto,
    net::SocketAddr,
    ops::Deref,
    sync::Arc,
    time::Duration,
};

enum HandshakeMsg {
    A(u64),
    B(u64),
}

#[derive(Clone)]
struct SecureishNode {
    node: Arc<Node>,
    handshakes: Arc<RwLock<HashMap<SocketAddr, HashSet<u64>>>>,
}

impl Deref for SecureishNode {
    type Target = Node;

    fn deref(&self) -> &Self::Target {
        &self.node
    }
}

impl ContainsNode for SecureishNode {
    fn node(&self) -> &Arc<Node> {
        &self.node
    }
}

impl_messaging_protocol!(SecureishNode);
impl PacketingProtocol for SecureishNode {}

impl HandshakeProtocol for SecureishNode {
    fn enable_handshake_protocol(&self) {
        let initiator = |addr: SocketAddr,
                         mut connection_reader: ConnectionReader,
                         connection: Arc<Connection>|
         -> JoinHandle<ConnectionReader> {
            tokio::spawn(async move {
                let node = Arc::clone(&connection_reader.node);
                debug!(parent: node.span(), "spawned a task to handshake with {}", addr);

                // send A
                if let Err(e) = connection
                    .write_bytes(&[HandshakeMsg::A as u8]) // TODO: send nonce too
                    .await
                {
                    error!(parent: node.span(), "can't send handshake message A to {}: {}", addr, e);
                // TODO: kick, register failure
                } else {
                    debug!(parent: node.span(), "sent handshake message A to {}", addr);
                };

                // read B
                match connection_reader.read_bytes(1).await {
                    Ok(1) => {
                        node.register_received_message(addr, 1);

                        if connection_reader.buffer[0] == HandshakeMsg::B as u8 {
                            // TODO: register nonce
                            debug!(parent: node.span(), "received handshake message B from {}", addr);
                        } else {
                            error!(parent: node.span(), "received an invalid handshake message from {} (expected B)", addr);
                            // TODO: kick, register failure
                        }
                    }
                    _ => {
                        // TODO: kick, register failure
                        error!(parent: node.span(), "couldn't read handshake message B");
                    }
                }

                connection_reader
            })
        };

        let responder = |addr: SocketAddr,
                         mut connection_reader: ConnectionReader,
                         connection: Arc<Connection>|
         -> JoinHandle<ConnectionReader> {
            tokio::spawn(async move {
                let node = Arc::clone(&connection_reader.node);
                debug!(parent: node.span(), "spawned a task to handshake with {}", addr);

                // read A
                match connection_reader.read_bytes(1).await {
                    Ok(1) => {
                        node.register_received_message(addr, 1);

                        if connection_reader.buffer[0] == HandshakeMsg::A as u8 {
                            // TODO: register nonce
                            debug!(parent: node.span(), "received handshake message A from {}", addr);
                        } else {
                            error!(parent: node.span(), "received an invalid handshake message from {} (expected A)", addr);
                            // TODO: kick, register failure
                        }
                    }
                    _ => {
                        // TODO: kick, register failure
                        error!(parent: node.span(), "couldn't read handshake message A");
                    }
                }

                // send B
                if let Err(e) = connection
                    .write_bytes(&[HandshakeMsg::B as u8]) // TODO: send nonce too
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

        self.node().set_handshake_closures(handshake_closures);
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

    // not required for the handshake; it's enabled only so that its relationship with the
    // handshake protocol can be tested too; it should kick in only after the handshake
    initiator_node.enable_messaging_protocol();
    responder_node.enable_messaging_protocol();

    initiator_node.enable_handshake_protocol();
    responder_node.enable_handshake_protocol();

    initiator_node
        .node
        .initiate_connection(responder_node.listening_addr)
        .await
        .unwrap();

    sleep(Duration::from_millis(100)).await;

    assert!(initiator_node.is_handshaken(responder_node.listening_addr));
    // TODO: the responder is also handshaken, but it knows the initiator under a different address;
    // obtain it and then perform the assertion
}
