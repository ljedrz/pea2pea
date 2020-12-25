use tokio::{io::AsyncReadExt, sync::mpsc::channel, task::JoinHandle, time::sleep};
use tracing::*;

mod common;
use pea2pea::{
    Connection, ConnectionReader, ContainsNode, HandshakeProtocol, HandshakeSetup, HandshakeState,
    MessagingProtocol, Node, NodeConfig,
};

use parking_lot::RwLock;
use std::{
    collections::HashMap,
    convert::TryInto,
    io::{self, ErrorKind},
    net::SocketAddr,
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
struct NoncePair(u64, u64); // (mine, peer's)

#[derive(Clone)]
struct SecureishNode {
    node: Arc<Node>,
    handshakes: Arc<RwLock<HashMap<SocketAddr, NoncePair>>>,
}

impl ContainsNode for SecureishNode {
    fn node(&self) -> &Arc<Node> {
        &self.node
    }
}

macro_rules! read_handshake_message {
    ($expected: path, $node: expr, $connection_reader: expr, $addr: expr) => {
        match $connection_reader.read_bytes(9).await {
            Ok(9) => {
                $node.register_received_message($addr, 9);
                let msg = HandshakeMsg::deserialize(&$connection_reader.buffer[..9]);

                if let $expected(nonce) = msg {
                    debug!(parent: $node.span(), "received handshake message B from {}", $addr);
                    nonce
                } else {
                    error!(parent: $node.span(), "received an invalid handshake message from {} (expected B)", $addr);
                    return Err(ErrorKind::Other.into());
                }
            }
            _ => {
                error!(parent: $node.span(), "couldn't read handshake message B");
                return Err(ErrorKind::Other.into());
            }
        }
    }
}

macro_rules! send_handshake_message {
    ($msg: expr, $node: expr, $connection: expr, $addr: expr) => {
        if let Err(e) = $connection
            .write_bytes(&$msg.serialize())
            .await
        {
            error!(parent: $node.span(), "can't send handshake message A to {}: {}", $addr, e);
            return Err(ErrorKind::Other.into());
        } else {
            debug!(parent: $node.span(), "sent handshake message A to {}", $addr);
        };
    }
}

impl_messaging_protocol!(SecureishNode);

impl HandshakeProtocol for SecureishNode {
    fn enable_handshake_protocol(&self) {
        let (state_sender, mut state_receiver) = channel::<(SocketAddr, HandshakeState)>(64);

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
         -> JoinHandle<io::Result<(ConnectionReader, HandshakeState)>> {
            tokio::spawn(async move {
                let node = Arc::clone(&connection_reader.node);
                debug!(parent: node.span(), "spawned a task to handshake with {}", addr);

                // send A
                let own_nonce = 0;
                send_handshake_message!(HandshakeMsg::A(own_nonce), node, connection, addr);

                // read B
                let peer_nonce =
                    read_handshake_message!(HandshakeMsg::B, node, connection_reader, addr);

                let nonce_pair = NoncePair(own_nonce, peer_nonce);

                Ok((connection_reader, Box::new(nonce_pair) as HandshakeState))
            })
        };

        let responder = |addr: SocketAddr,
                         mut connection_reader: ConnectionReader,
                         connection: Arc<Connection>|
         -> JoinHandle<io::Result<(ConnectionReader, HandshakeState)>> {
            tokio::spawn(async move {
                let node = Arc::clone(&connection_reader.node);
                debug!(parent: node.span(), "spawned a task to handshake with {}", addr);

                // read A
                let peer_nonce =
                    read_handshake_message!(HandshakeMsg::A, node, connection_reader, addr);

                // send B
                let own_nonce = 1;
                send_handshake_message!(HandshakeMsg::B(own_nonce), node, connection, addr);

                let nonce_pair = NoncePair(own_nonce, peer_nonce);

                Ok((connection_reader, Box::new(nonce_pair) as HandshakeState))
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
async fn handshake_protocol() {
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
    initiator.enable_messaging_protocol();
    responder.enable_messaging_protocol();

    initiator.enable_handshake_protocol();
    responder.enable_handshake_protocol();

    initiator
        .node
        .initiate_connection(responder.node().listening_addr)
        .await
        .unwrap();

    sleep(Duration::from_millis(100)).await;

    assert!(initiator.node().handshaken_addrs().len() == 1);
    assert!(responder.node().handshaken_addrs().len() == 1);

    assert!(initiator.handshakes.read().values().next() == Some(&NoncePair(0, 1)));
    assert!(responder.handshakes.read().values().next() == Some(&NoncePair(1, 0)));
}
