//! Opt-in protocols available to the `Node`; each protocol is expected to spawn its own task that runs throughout the
//! `Node`'s lifetime and handles a specific functionality. The communication with these tasks is done via
//! `ProtocolHandler`s.

use crate::connections::Connection;

use once_cell::sync::OnceCell;
use tokio::sync::{mpsc, oneshot};

use std::io;

mod handshaking;
mod reading;
mod writing;

pub use handshaking::Handshaking;
pub use reading::Reading;
pub use writing::Writing;

#[derive(Default)]
pub(crate) struct Protocols {
    pub(crate) handshake_handler: OnceCell<ProtocolHandler>,
    pub(crate) reading_handler: OnceCell<ProtocolHandler>,
    pub(crate) writing_handler: OnceCell<ProtocolHandler>,
}

/// An object dedicated to managing a protocol; it contains a `Sender` whose other side is
/// owned by the protocol's task.
pub struct ProtocolHandler {
    sender: mpsc::Sender<ReturnableConnection>,
}

impl ProtocolHandler {
    /// Sends a returnable `Connection` to a task spawned by the protocol handler.
    pub async fn send(&self, returnable_conn: ReturnableConnection) {
        if self.sender.send(returnable_conn).await.is_err() {
            unreachable!(); // protocol's task is down! can't recover
        }
    }
}

impl From<mpsc::Sender<ReturnableConnection>> for ProtocolHandler {
    fn from(sender: mpsc::Sender<ReturnableConnection>) -> Self {
        Self { sender }
    }
}

/// An object allowing a `Connection` to be "borrowed" from the owning `Node` to enable a protocol
/// and to be sent back to it once it's done its job.
pub type ReturnableConnection = (Connection, oneshot::Sender<io::Result<Connection>>);
