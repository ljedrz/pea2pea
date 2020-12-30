use once_cell::sync::OnceCell;

mod handshaking;
mod messaging;

pub use handshaking::{HandshakeHandler, HandshakeObjects, Handshaking};
pub use messaging::{InboundHandler, Messaging};

#[derive(Default)]
pub(crate) struct Protocols {
    pub(crate) inbound_handler: OnceCell<InboundHandler>,
    pub(crate) handshake_handler: OnceCell<HandshakeHandler>,
}
