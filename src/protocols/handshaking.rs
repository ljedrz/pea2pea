use crate::{Connection, ConnectionReader};

use tokio::{sync::mpsc::Sender, task::JoinHandle};

use std::{any::Any, io, net::SocketAddr, sync::Arc};

pub trait Handshaking {
    // prepare the Node to produce and handle handshakes
    fn enable_handshaking(&self);
}

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

// FIXME: the pub for members is not ideal
pub struct HandshakeSetup {
    pub initiator_closure: HandshakeClosure,
    pub responder_closure: HandshakeClosure,
    pub state_sender: Option<Sender<(SocketAddr, HandshakeState)>>,
}
