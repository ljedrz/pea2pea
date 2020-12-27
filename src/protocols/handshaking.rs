use crate::{Connection, ConnectionReader};

use tokio::{sync::mpsc::Sender, task::JoinHandle};

use std::{any::Any, io, net::SocketAddr, sync::Arc};

/// This protocol can be used to specify and enable network handshakes. Upon establishing a connection, both sides will
/// need to adhere to the specified handshake rules in order to finalize the connection and be able to transmit any
/// messages. Note: if not implemented, the nodes unconditionally mark their connections as handshaken.
pub trait Handshaking {
    /// Prepares the node to produce and handle network handshakes.
    fn enable_handshaking(&self);
}

/// A trait object containing handshake state.
pub type HandshakeState = Box<dyn Any + Send>;

// FIXME; simplify
type HandshakeClosure = Box<
    dyn Fn(
            SocketAddr,
            ConnectionReader,
            Arc<Connection>,
        ) -> JoinHandle<io::Result<(ConnectionReader, HandshakeState)>>
        + Send
        + Sync,
>;

/// An object dedicated to handling connection handshakes.
pub struct HandshakeSetup {
    /// The closure for performing handshakes as the connection initiator.
    pub initiator_closure: HandshakeClosure,
    /// The closure for performing handshakes as the connection responder.
    pub responder_closure: HandshakeClosure,
    /// Can be used to persist any handshake state.
    pub state_sender: Option<Sender<(SocketAddr, HandshakeState)>>,
}
