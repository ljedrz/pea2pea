use crate::{connection::ConnectionReader, Node, ReadProtocol};

use tokio::task::JoinHandle;

use std::{net::SocketAddr, sync::Arc};

pub trait HandshakeProtocol: ReadProtocol {
    // prepare the Node to produce and handle handshakes
    fn enable_handshake_protocol(&self);
}

// FIXME: the pub for members is not ideal
pub struct HandshakeClosures {
    // FIXME; simplify; also, should prolly return some sort of a Result
    pub initiator: Box<
        dyn Fn(Arc<Node>, SocketAddr, ConnectionReader) -> JoinHandle<ConnectionReader>
            + Send
            + Sync
            + 'static,
    >,
    pub responder: Box<
        dyn Fn(Arc<Node>, SocketAddr, ConnectionReader) -> JoinHandle<ConnectionReader>
            + Send
            + Sync
            + 'static,
    >,
}
