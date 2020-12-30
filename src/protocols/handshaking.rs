use crate::{Connection, ConnectionReader};

use tokio::{sync::mpsc::Sender, task::JoinHandle};

use std::{any::Any, io, net::SocketAddr};

/// This protocol can be used to specify and enable network handshakes. Upon establishing a connection, both sides will
/// need to adhere to the specified handshake rules in order to finalize the connection and be able to transmit any
/// messages.
pub trait Handshaking {
    /// Prepares the node to produce and handle network handshakes.
    fn enable_handshaking(&self);
}

/// A trait object containing the result of a handshake, if there's any.
pub type HandshakeResult = Box<dyn Any + Send>;

// FIXME; simplify
type HandshakeClosure = Box<
    dyn Fn(
            ConnectionReader,
            Connection,
        ) -> JoinHandle<io::Result<(ConnectionReader, Connection, HandshakeResult)>>
        + Send
        + Sync,
>;

/// An object dedicated to handling connection handshakes.
pub struct HandshakeSetup {
    /// The closure for performing handshakes as the connection initiator.
    pub initiator_closure: HandshakeClosure,
    /// The closure for performing handshakes as the connection responder.
    pub responder_closure: HandshakeClosure,
    /// Can be used to persist any handshake result.
    pub result_sender: Option<Sender<(SocketAddr, HandshakeResult)>>,
}
