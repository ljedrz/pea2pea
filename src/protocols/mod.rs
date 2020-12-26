use once_cell::sync::OnceCell;

mod broadcasting;
mod handshaking;
mod maintenance;
mod messaging;

pub use broadcasting::BroadcastProtocol;
pub use handshaking::{HandshakeProtocol, HandshakeSetup, HandshakeState};
pub use maintenance::MaintenanceProtocol;
pub use messaging::{InboundMessage, MessagingProtocol, ReadingClosure};

pub(crate) use messaging::InboundMessages;

#[derive(Default)]
pub(crate) struct Protocols {
    pub(crate) reading_closure: OnceCell<ReadingClosure>,
    pub(crate) inbound_messages: OnceCell<InboundMessages>,
    pub(crate) handshake_setup: OnceCell<HandshakeSetup>,
}
