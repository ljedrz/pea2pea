use once_cell::sync::OnceCell;

mod broadcasting;
mod handshaking;
mod maintenance;
mod messaging;
mod packeting;

pub use broadcasting::BroadcastProtocol;
pub use handshaking::{HandshakeProtocol, HandshakeSetup, HandshakeState};
pub use maintenance::MaintenanceProtocol;
pub use messaging::{InboundMessage, MessagingClosure, MessagingProtocol};
pub use packeting::{PacketingClosure, PacketingProtocol};

pub(crate) use messaging::InboundMessages;

#[derive(Default)]
pub(crate) struct Protocols {
    pub(crate) messaging_closure: OnceCell<MessagingClosure>,
    pub(crate) packeting_closure: OnceCell<PacketingClosure>,
    pub(crate) inbound_messages: OnceCell<InboundMessages>,
    pub(crate) handshake_setup: OnceCell<HandshakeSetup>,
}
