use tokio::{io::AsyncReadExt, sync::mpsc::channel, task::JoinHandle, time::sleep};
use tracing::*;

mod common;
use pea2pea::{
    Connection, ConnectionReader, ContainsNode, DynHandshakeState, HandshakeProtocol,
    HandshakeSetup, MessagingProtocol, Node, NodeConfig, PacketingProtocol,
};

use parking_lot::RwLock;
use std::{
    collections::HashMap,
    convert::TryInto,
    io::{self, ErrorKind},
    net::SocketAddr,
    ops::Deref,
    sync::Arc,
    time::Duration,
};

enum HandshakeMsg {
    A(u64),
    B(u64),
}

impl HandshakeMsg {
    fn deserialize(bytes: &[u8]) -> Self {
        let value = u64::from_le_bytes(bytes[1..9].try_into().unwrap());

        match bytes[0] {
            0 => HandshakeMsg::A(value),
            1 => HandshakeMsg::B(value),
            _ => unreachable!(),
        }
    }

    fn serialize(&self) -> Vec<u8> {
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

        ret
    }
}

#[derive(PartialEq, Eq)]
struct NoncePair {
    mine: u64,
    peers: u64,
}

#[derive(Clone)]
struct SecureishNode {
    node: Arc<Node>,
    handshakes: Arc<RwLock<HashMap<SocketAddr, NoncePair>>>,
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
        let (state_sender, mut state_receiver) = channel::<(SocketAddr, DynHandshakeState)>(64);

        let self_clone = self.clone();
        tokio::spawn(async move {
            loop {
                if let Some((addr, state)) = state_receiver.recv().await {
                    let state = state.downcast().unwrap();
                    self_clone.handshakes.write().insert(addr, *state);
                }
            }
        });

        let initiator = |addr: SocketAddr,
                         mut connection_reader: ConnectionReader,
                         connection: Arc<Connection>|
         -> JoinHandle<io::Result<(ConnectionReader, DynHandshakeState)>> {
            tokio::spawn(async move {
                let node = Arc::clone(&connection_reader.node);
                debug!(parent: node.span(), "spawned a task to handshake with {}", addr);

                // send A
                let own_nonce = 0;
                if let Err(e) = connection
                    .write_bytes(&HandshakeMsg::A(own_nonce).serialize())
                    .await
                {
                    error!(parent: node.span(), "can't send handshake message A to {}: {}", addr, e);
                    return Err(ErrorKind::Other.into());
                } else {
                    debug!(parent: node.span(), "sent handshake message A to {}", addr);
                };

                // read B
                let peer_nonce = match connection_reader.read_bytes(9).await {
                    Ok(9) => {
                        node.register_received_message(addr, 9);
                        let msg = HandshakeMsg::deserialize(&connection_reader.buffer[..9]);

                        if let HandshakeMsg::B(peer_nonce) = msg {
                            debug!(parent: node.span(), "received handshake message B from {}", addr);
                            peer_nonce
                        } else {
                            error!(parent: node.span(), "received an invalid handshake message from {} (expected B)", addr);
                            return Err(ErrorKind::Other.into());
                        }
                    }
                    _ => {
                        error!(parent: node.span(), "couldn't read handshake message B");
                        return Err(ErrorKind::Other.into());
                    }
                };

                let nonce_pair = NoncePair {
                    mine: own_nonce,
                    peers: peer_nonce,
                };

                Ok((connection_reader, Box::new(nonce_pair) as DynHandshakeState))
            })
        };

        let responder = |addr: SocketAddr,
                         mut connection_reader: ConnectionReader,
                         connection: Arc<Connection>|
         -> JoinHandle<io::Result<(ConnectionReader, DynHandshakeState)>> {
            tokio::spawn(async move {
                let node = Arc::clone(&connection_reader.node);
                debug!(parent: node.span(), "spawned a task to handshake with {}", addr);

                // read A
                let peer_nonce = match connection_reader.read_bytes(9).await {
                    Ok(9) => {
                        node.register_received_message(addr, 9);
                        let msg = HandshakeMsg::deserialize(&connection_reader.buffer[..9]);

                        if let HandshakeMsg::A(peer_nonce) = msg {
                            debug!(parent: node.span(), "received handshake message A from {}", addr);
                            peer_nonce
                        } else {
                            error!(parent: node.span(), "received an invalid handshake message from {} (expected A)", addr);
                            return Err(ErrorKind::Other.into());
                        }
                    }
                    _ => {
                        error!(parent: node.span(), "couldn't read handshake message A");
                        return Err(ErrorKind::Other.into());
                    }
                };

                // send B
                let own_nonce = 1;
                if let Err(e) = connection
                    .write_bytes(&HandshakeMsg::B(own_nonce).serialize())
                    .await
                {
                    error!(parent: node.span(), "can't send handshake message B to {}: {}", addr, e);
                } else {
                    debug!(parent: node.span(), "sent handshake message B to {}", addr);
                };

                let nonce_pair = NoncePair {
                    mine: own_nonce,
                    peers: peer_nonce,
                };

                Ok((connection_reader, Box::new(nonce_pair) as DynHandshakeState))
            })
        };

        let handshake_setup = HandshakeSetup {
            initiator_closure: Box::new(initiator),
            responder_closure: Box::new(responder),
            state_sender: Some(state_sender),
        };

        self.node().set_handshake_setup(handshake_setup);
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

    assert!(initiator_node.handshaken_addrs().len() == 1);
    assert!(responder_node.handshaken_addrs().len() == 1);

    assert!(
        initiator_node.handshakes.read().values().next() == Some(&NoncePair { mine: 0, peers: 1 })
    );
    assert!(
        responder_node.handshakes.read().values().next() == Some(&NoncePair { mine: 1, peers: 0 })
    );
}
