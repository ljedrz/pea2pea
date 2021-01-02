use crate::{protocols::ReturnableConnection, Pea2Pea};

use tokio::sync::mpsc;

/// Can be used to specify and enable network handshakes. Upon establishing a connection, both sides will
/// need to adhere to the specified handshake rules in order to finalize the connection and be able to send
/// or receive any messages.
pub trait Handshaking: Pea2Pea {
    /// Prepares the node to perform specified network handshakes.
    fn enable_handshaking(&self);
}

/// An object dedicated to handling connection handshakes; used in the `Handshaking` protocol.
pub struct HandshakeHandler(mpsc::Sender<ReturnableConnection>);

impl HandshakeHandler {
    /// Sends a returnable `Connection` to a task spawned by the `HandshakeHandler`.
    pub async fn send(&self, returnable_conn: ReturnableConnection) {
        if self.0.send(returnable_conn).await.is_err() {
            // can't recover if this happens
            panic!("HandshakeHandler's Receiver is closed")
        }
    }
}

impl From<mpsc::Sender<ReturnableConnection>> for HandshakeHandler {
    fn from(sender: mpsc::Sender<ReturnableConnection>) -> Self {
        Self(sender)
    }
}
