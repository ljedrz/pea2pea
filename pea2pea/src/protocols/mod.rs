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
    task::JoinSet,
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

/// Awaits a response from a protocol handler task, with a backstop against a message
/// being stranded in the handler's channel. The graceful shutdown drain in the handler
/// loops normally guarantees no message is stranded (draining via `recv` waits out any
/// mid-write send); this guard only matters when the drain is bypassed - i.e. when the
/// handler task gets hard-aborted because its drain timed out or a `shut_down` future
/// was dropped mid-execution. Dropping a channel's receiver only drains the values
/// queued up to that point, so a value whose write completes just after that drain is
/// stranded in the closed channel, where the response sender within it is never used
/// nor dropped; polling the channel's closed flag converts what would otherwise be a
/// permanently wedged await into a bounded one.
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

/// Runs a protocol handler's main loop for the setup-oriented protocols
/// (`Handshake`/`Reading`/`Writing`): every received request is spawned as a
/// tracked setup task via `spawn_setup`. On the node's shutdown signal the
/// loop stops accepting new requests and fails the queued ones instead of
/// spawning them: dropping an item drops the returner within, which the
/// caller observes as a clean "shutting down". Draining via `recv` (as
/// opposed to dropping the receiver) waits out any send that is still
/// mid-write, so no message can be stranded in the channel.
pub(crate) async fn run_setup_handler_loop<T: Send, U: Send>(
    node: Node,
    protocol: &'static str,
    mut receiver: mpsc::Receiver<ReturnableItem<T, U>>,
    mut spawn_setup: impl FnMut(ReturnableItem<T, U>, &mut JoinSet<()>) + Send,
) {
    // tracks all in-flight setup tasks
    let mut setup_tasks = JoinSet::new();
    let mut shutdown = node.shutdown_signal.subscribe();
    let mut draining = false;

    loop {
        tokio::select! {
            biased;
            // task set cleanups
            res = setup_tasks.join_next(), if !setup_tasks.is_empty() => {
                log_setup_join(node.span(), protocol, res);
            }
            maybe_item = receiver.recv() => {
                match maybe_item {
                    Some(_item) if draining => {} // fail the queued setups
                    Some(item) => spawn_setup(item, &mut setup_tasks),
                    None => break, // channel closed and drained
                }
            }
            _ = shutdown.wait_for(|sig| *sig), if !draining => {
                receiver.close();
                draining = true;
            }
        }
    }
}

/// Runs a protocol handler's main loop for the hook-oriented protocols
/// (`OnConnect`/`OnDisconnect`): every received trigger is handled via
/// `process`. On the node's shutdown signal the loop stops accepting new
/// triggers, but still runs the queued ones - they belong to connections
/// that made it into the active set, so skipping them would break the
/// `OnConnect`/`OnDisconnect` pairing; draining via `recv` also waits out
/// any trigger that is still mid-send, so none can be stranded in the
/// channel.
pub(crate) async fn run_hook_handler_loop<T: Send, U: Send>(
    node: Node,
    mut receiver: mpsc::Receiver<ReturnableItem<T, U>>,
    mut process: impl FnMut(ReturnableItem<T, U>) + Send,
) {
    let mut shutdown = node.shutdown_signal.subscribe();
    let mut draining = false;

    loop {
        tokio::select! {
            biased;
            maybe_item = receiver.recv() => {
                match maybe_item {
                    Some(item) => process(item),
                    None => break, // channel closed and drained
                }
            }
            _ = shutdown.wait_for(|sig| *sig), if !draining => {
                receiver.close();
                draining = true;
            }
        }
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
