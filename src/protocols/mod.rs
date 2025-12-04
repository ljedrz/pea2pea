//! Opt-in protocols available to the node; each protocol is expected to spawn its own task that runs throughout the
//! node's lifetime and handles a specific functionality. The communication with these tasks is done via dedicated
//! handler objects.
//!
//! A flowchart detailing how the protocols interact with a connection during its lifetime can be seen
//! [here](https://github.com/ljedrz/pea2pea/tree/master/assets/connection_lifetime.png).

use std::{io, net::SocketAddr, sync::OnceLock};

use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use crate::{connections::Connection, node::Node};

mod handshake;
mod on_connect;
mod on_disconnect;
mod reading;
mod writing;

pub use handshake::Handshake;
pub use on_connect::OnConnect;
pub use on_disconnect::OnDisconnect;
pub use reading::Reading;
pub use writing::Writing;

// The value returned to the node by the OnDisconnect protocol is a bit complex,
// so use an alias to break it down.
type OnDisconnectBundle = (JoinHandle<()>, oneshot::Receiver<()>);

#[derive(Default)]
pub(crate) struct Protocols {
    pub(crate) handshake: OnceLock<ProtocolHandler<Connection, io::Result<Connection>>>,
    pub(crate) reading: OnceLock<ProtocolHandler<Connection, io::Result<Connection>>>,
    pub(crate) writing: OnceLock<writing::WritingHandler>,
    pub(crate) on_connect: OnceLock<ProtocolHandler<SocketAddr, JoinHandle<()>>>,
    pub(crate) on_disconnect: OnceLock<ProtocolHandler<SocketAddr, OnDisconnectBundle>>,
}

/// An object sent to a protocol handler task; the task assumes control of a protocol-relevant item `T`,
/// and when it's done with it, it returns it (possibly in a wrapper object) or another relevant object
/// to the callsite via the counterpart [`oneshot::Receiver`].
pub(crate) type ReturnableItem<T, U> = (T, oneshot::Sender<U>);

pub(crate) type ReturnableConnection = ReturnableItem<Connection, io::Result<Connection>>;

pub(crate) struct ProtocolHandler<T, U>(mpsc::Sender<ReturnableItem<T, U>>);

pub(crate) trait Protocol<T, U> {
    async fn trigger(&self, item: ReturnableItem<T, U>);
}

impl<T, U> Protocol<T, U> for ProtocolHandler<T, U> {
    async fn trigger(&self, item: ReturnableItem<T, U>) {
        // ignore errors; they can only happen if a disconnect interrupts the protocol setup process
        let _ = self.0.send(item).await;
    }
}

/// This object is used to ensure that the related peer is going to be disconnected from
/// even if the owning task panics due to a user implementation error.
pub(crate) struct DisconnectOnDrop {
    pub(crate) node: Option<Node>,
    pub(crate) addr: SocketAddr,
}

impl DisconnectOnDrop {
    pub(crate) fn new(node: Node, addr: SocketAddr) -> Self {
        Self {
            node: Some(node),
            addr,
        }
    }
}

impl Drop for DisconnectOnDrop {
    fn drop(&mut self) {
        if let Some(node) = self.node.take() {
            let addr = self.addr;
            // can't await inside Drop.
            tokio::spawn(async move { node.disconnect(addr).await });
        }
    }
}
