use once_cell::sync::OnceCell;

mod handshaking;
mod messaging;

pub use handshaking::{HandshakeResult, HandshakeSetup, Handshaking};
pub use messaging::{Messaging, ReadingClosure};

pub(crate) use messaging::InboundMessages;

#[derive(Default)]
pub(crate) struct Protocols {
    pub(crate) reading_closure: OnceCell<ReadingClosure>,
    pub(crate) inbound_messages: OnceCell<InboundMessages>,
    pub(crate) handshake_setup: OnceCell<HandshakeSetup>,
}
