//! Opt-in protocols available to the node; each protocol is expected to spawn its own task that runs throughout the
//! node's lifetime and handles a specific functionality. The communication with these tasks is done via dedicated
//! handler objects.
//!
//! A flowchart detailing how the protocols interact with a connection during its lifetime can be seen
//! [here](https://github.com/ljedrz/pea2pea/tree/master/assets/connection_lifetime.png).

use std::{
    io,
    net::SocketAddr,
    sync::{OnceLock, atomic::Ordering},
    time::Duration,
};

use tokio::{
    sync::{mpsc, oneshot},
    time::timeout,
};

use crate::{
    connections::{Connection, DisconnectOrigin},
    node::Node,
    protocols::{on_connect::OnConnectBundle, on_disconnect::OnDisconnectBundle},
};

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

#[derive(Default)]
pub(crate) struct Protocols {
    pub(crate) handshake: OnceLock<ProtocolHandler<Connection, io::Result<Connection>>>,
    pub(crate) reading: OnceLock<ProtocolHandler<Connection, io::Result<Connection>>>,
    pub(crate) writing: OnceLock<writing::WritingHandler>,
    pub(crate) on_connect: OnceLock<ProtocolHandler<(SocketAddr, u64), OnConnectBundle>>,
    pub(crate) on_disconnect:
        OnceLock<ProtocolHandler<(SocketAddr, DisconnectOrigin), OnDisconnectBundle>>,
}

/// An object sent to a protocol handler task; the task assumes control of a protocol-relevant item `T`,
/// and when it's done with it, it returns it (possibly in a wrapper object) or another relevant object
/// to the callsite via the counterpart [`oneshot::Receiver`].
pub(crate) type ReturnableItem<T, U> = (T, oneshot::Sender<U>);

pub(crate) type ReturnableConnection = ReturnableItem<Connection, io::Result<Connection>>;

pub(crate) struct ProtocolHandler<T, U>(mpsc::Sender<ReturnableItem<T, U>>);

impl<T, U> ProtocolHandler<T, U> {
    /// Indicates whether the handler's channel has been closed, i.e. its task is gone.
    pub(crate) fn is_closed(&self) -> bool {
        self.0.is_closed()
    }
}

/// Awaits a response from a protocol handler task, guarding against a rare race in which
/// the handler task is aborted (during node shutdown) while the triggering message is
/// still being written into its bounded channel: dropping the channel's receiver drains
/// the values queued up to that point, but a value whose write completes just after that
/// drain is stranded in the closed channel, where the response sender within it is never
/// used nor dropped. Polling the channel's closed flag converts what would otherwise be
/// a permanently wedged await into a bounded one.
///
/// Returns `None` when the handler is gone (i.e. the node is shutting down); note that
/// the message - along with anything it owns - may remain queued in the closed channel
/// until the node is fully dropped.
pub(crate) async fn await_handler_response<U>(
    mut receiver: oneshot::Receiver<U>,
    handler_is_closed: impl Fn() -> bool,
) -> Option<U> {
    loop {
        match timeout(Duration::from_millis(500), &mut receiver).await {
            // the handler responded
            Ok(Ok(response)) => return Some(response),
            // the message (and the response sender within it) was dropped cleanly
            Ok(Err(_)) => return None,
            // no response yet; if the handler's channel is closed, the message is stranded
            Err(_) if handler_is_closed() => return None,
            // still legitimately in flight (e.g. an ongoing handshake); keep waiting
            Err(_) => {}
        }
    }
}

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
    /// The id of the specific connection instance this guard belongs to. On drop the guard only
    /// acts if the address is still occupied by that same instance; otherwise a connection that
    /// reused the address after a rapid reconnect would be torn down by a stale guard.
    pub(crate) conn_id: u64,
    pub(crate) origin: DisconnectOrigin,
}

impl DisconnectOnDrop {
    pub(crate) fn new(
        node: Node,
        addr: SocketAddr,
        conn_id: u64,
        origin: DisconnectOrigin,
    ) -> Self {
        Self {
            node: Some(node),
            addr,
            conn_id,
            origin,
        }
    }
}

impl Drop for DisconnectOnDrop {
    fn drop(&mut self) {
        if let Some(node) = self.node.take() {
            let (addr, conn_id, origin) = (self.addr, self.conn_id, self.origin);
            // only recover if the address is still held by *our* connection instance and it isn't
            // already being disconnected; a different id means a newer connection reused the
            // address and must not be touched by this defunct connection's guard. This check only
            // avoids a needless spawn - the address could still be reused before the spawned task
            // runs, so the id is passed along and re-checked atomically with the disconnect claim
            let needs_recovery = node
                .connections
                .active
                .read()
                .get(&addr)
                .is_some_and(|c| c.id == conn_id && !c.disconnecting.load(Ordering::Acquire));
            if needs_recovery && let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(
                    async move { node.disconnect_w_origin(addr, origin, Some(conn_id)).await },
                );
            }
        }
    }
}

pub(crate) fn log_setup_join(
    span: &tracing::Span,
    protocol: &'static str,
    res: Option<Result<(), tokio::task::JoinError>>,
) {
    if let Some(Err(e)) = res
        && e.is_panic()
    {
        tracing::error!(parent: span, "a {protocol} setup task panicked: {e}");
    }
}

/// Extracts a human-readable message from a panic payload caught via `catch_unwind`.
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<&'static str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("<non-string panic payload>")
}
