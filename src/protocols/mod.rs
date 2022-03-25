//! Opt-in protocols available to the node; each protocol is expected to spawn its own task that runs throughout the
//! node's lifetime and handles a specific functionality. The communication with these tasks is done via dedicated
//! handler objects.

use crate::connections::Connection;

use once_cell::sync::OnceCell;
use tokio::sync::{mpsc, oneshot};

use std::{io, net::SocketAddr};

mod disconnect;
mod handshake;
mod reading;
mod writing;

pub use disconnect::Disconnect;
pub use handshake::Handshake;
pub use reading::Reading;
pub use writing::Writing;

#[derive(Default)]
pub(crate) struct Protocols {
    pub(crate) handshake_handler: OnceCell<ProtocolHandler<Connection, io::Result<Connection>>>,
    pub(crate) reading_handler: OnceCell<ProtocolHandler<Connection, io::Result<Connection>>>,
    pub(crate) writing_handler: OnceCell<writing::WritingHandler>,
    pub(crate) disconnect_handler: OnceCell<ProtocolHandler<SocketAddr, ()>>,
}

/// An object sent to a protocol handler task; the task assumes control of a protocol-relevant item `T`,
/// and when it's done with it, it returns it (possibly in a wrapper object) or another relevant object
/// to the callsite via the counterpart [`oneshot::Receiver`].
pub(crate) type ReturnableItem<T, U> = (T, oneshot::Sender<U>);

pub(crate) type ReturnableConnection = ReturnableItem<Connection, io::Result<Connection>>;

pub(crate) struct ProtocolHandler<T, U>(mpsc::UnboundedSender<ReturnableItem<T, U>>);

pub(crate) trait Protocol<T, U> {
    fn trigger(&self, item: ReturnableItem<T, U>);
}

impl<T, U> Protocol<T, U> for ProtocolHandler<T, U> {
    fn trigger(&self, item: ReturnableItem<T, U>) {
        if self.0.send(item).is_err() {
            // ignore: this can only happen if a disconnect interrupts the protocol setup process
        }
    }
}
