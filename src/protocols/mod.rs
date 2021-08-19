//! Opt-in protocols available to the `Node`; each protocol is expected to spawn its own task that runs throughout the
//! `Node`'s lifetime and handles a specific functionality. The communication with these tasks is done via
//! `ProtocolHandler`s.

use crate::connections::Connection;

use once_cell::sync::OnceCell;
use tokio::sync::oneshot;

use std::io;

mod disconnect;
mod handshaking;
mod reading;
mod writing;

pub use disconnect::{Disconnect, DisconnectHandler};
pub use handshaking::{Handshaking, HandshakingHandler};
pub use reading::{Reading, ReadingHandler};
pub use writing::{Writing, WritingHandler};

#[derive(Default)]
pub(crate) struct Protocols {
    pub(crate) handshake_handler: OnceCell<HandshakingHandler>,
    pub(crate) reading_handler: OnceCell<ReadingHandler>,
    pub(crate) writing_handler: OnceCell<WritingHandler>,
    pub(crate) disconnect_handler: OnceCell<DisconnectHandler>,
}

/// The basic functionality that every protocol is required to handle - being triggered.
#[async_trait::async_trait]
pub trait ProtocolHandler {
    /// The type of item that gets sent to the protocol handler task.
    type Item;

    /// Triggers the protocol's behavior for the given subject, usually a `Connection` or
    /// its `SocketAddr`.
    async fn trigger(&self, item: Self::Item);
}

/// An object sent to a protocol handler task; the task assumes control of a protocol-relevant item `T`,
/// and when it's done with it, it returns it (possibly in a wrapper object) or another relevant object
/// to the callsite via the counterpart `oneshot::Receiver`.
pub type ReturnableItem<T, U> = (T, oneshot::Sender<U>);

pub(crate) type ReturnableConnection = ReturnableItem<Connection, io::Result<Connection>>;
