//! Opt-in protocols available to the `Node`; each protocol is expected to spawn its own task that runs throughout the
//! `Node`'s lifetime and handles a specific functionality. The communication with these tasks is done via
//! `ProtocolHandler`s.

use crate::connections::Connection;

use once_cell::sync::OnceCell;
use tokio::sync::{mpsc, oneshot};

use std::{io, net::SocketAddr};

mod disconnect;
mod handshaking;
mod reading;
mod writing;

pub use disconnect::Disconnect;
pub use handshaking::Handshaking;
pub use reading::Reading;
pub use writing::Writing;

#[derive(Default)]
pub(crate) struct Protocols {
    pub(crate) handshake_handler: OnceCell<ProtocolHandler<ReturnableConnection>>,
    pub(crate) reading_handler: OnceCell<ProtocolHandler<ReturnableConnection>>,
    pub(crate) writing_handler: OnceCell<ProtocolHandler<ReturnableConnection>>,
    pub(crate) disconnect_handler: OnceCell<ProtocolHandler<(SocketAddr, oneshot::Sender<()>)>>,
}

/// An object dedicated to managing a protocol; it contains a `Sender` whose other side is
/// owned by the protocol's task.
pub struct ProtocolHandler<T: Send> {
    sender: mpsc::Sender<T>,
}

impl<T: Send> ProtocolHandler<T> {
    /// Sends a returnable item to a task spawned by the protocol handler.
    pub async fn send(&self, item: T) {
        if self.sender.send(item).await.is_err() {
            unreachable!(); // protocol's task is down! can't recover
        }
    }
}

impl<T: Send> From<mpsc::Sender<T>> for ProtocolHandler<T> {
    fn from(sender: mpsc::Sender<T>) -> Self {
        Self { sender }
    }
}

/// An object allowing a `Connection` to be "borrowed" from the owning `Node` to enable a protocol
/// and to be sent back to it once it's done its job.
pub type ReturnableConnection = (Connection, oneshot::Sender<io::Result<Connection>>);
