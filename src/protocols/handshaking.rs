use crate::{Connection, ConnectionReader, Pea2Pea};

use tokio::sync::{mpsc, oneshot};

use std::io;

/// This protocol can be used to specify and enable network handshakes. Upon establishing a connection, both sides will
/// need to adhere to the specified handshake rules in order to finalize the connection and be able to transmit any
/// messages.
pub trait Handshaking: Pea2Pea {
    /// Prepares the node to produce and handle network handshakes.
    fn enable_handshaking(&self);
}

/// A set of objects required to perform handshakes.
pub type HandshakeObjects = (
    ConnectionReader,
    Connection,
    oneshot::Sender<io::Result<(ConnectionReader, Connection)>>,
);

/// An object dedicated to handling connection handshakes.
pub struct HandshakeHandler(mpsc::Sender<HandshakeObjects>);

impl HandshakeHandler {
    /// Sends handshake-relevant objects to the handshake handler.
    pub async fn send(&self, handshake_objects: HandshakeObjects) {
        if self.0.send(handshake_objects).await.is_err() {
            // can't recover if this happens
            panic!("the handshake handling task is down or its Receiver is closed")
        }
    }
}

impl From<mpsc::Sender<HandshakeObjects>> for HandshakeHandler {
    fn from(sender: mpsc::Sender<HandshakeObjects>) -> Self {
        Self(sender)
    }
}
