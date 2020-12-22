use crate::{Connection, ConnectionReader};

use tokio::task::JoinHandle;

use std::{net::SocketAddr, sync::Arc};

pub trait HandshakeProtocol {
    // prepare the Node to produce and handle handshakes
    fn enable_handshake_protocol(&self);
}

// FIXME; simplify; also, should prolly return some sort of a Result
type HandshakeClosure = Box<
    dyn Fn(SocketAddr, ConnectionReader, Arc<Connection>) -> JoinHandle<ConnectionReader>
        + Send
        + Sync
        + 'static,
>;

// FIXME: the pub for members is not ideal
pub struct HandshakeClosures {
    pub initiator: HandshakeClosure,
    pub responder: HandshakeClosure,
}
