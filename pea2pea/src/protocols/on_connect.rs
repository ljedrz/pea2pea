use std::{
    future::Future,
    net::SocketAddr,
    panic::{AssertUnwindSafe, resume_unwind},
};

use futures_util::FutureExt;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tracing::*;

#[cfg(doc)]
use crate::{
    Connection, Node,
    protocols::{Handshake, OnDisconnect, Reading, Writing},
};
use crate::{
    Pea2Pea,
    connections::{DisconnectOrigin, create_connection_span},
    node::NodeTask,
    protocols::{
        DisconnectOnDrop, ProtocolHandler, install_protocol_handler, panic_message,
        run_hook_handler_loop,
    },
};

// The value returned to the node is a bit complex, so use an alias to break it down.
pub(crate) type OnConnectBundle = (JoinHandle<()>, bool);

/// Can be used to automatically perform some initial actions when a connection with a peer is
/// fully established. The reason for its existence (instead of including such behavior in the
/// [`Handshake`] impl) is that it allows the user to utilize [`Writing`] (if implemented and
/// enabled) and [`OnDisconnect`], while [`Handshake`] is very low-level and doesn't provide any
/// additional functionality - its purpose is only to establish a connection.
///
/// # Concurrency with the connection lifecycle
///
/// [`OnConnect::on_connect`] runs concurrently with the rest of the connection. The moment the
/// handshake concludes, the connection is "live": [`Reading`] starts consuming inbound messages,
/// [`Writing`] can be used to send them, and [`Node::disconnect`] - whether called explicitly
/// or triggered automatically by a read/write error or peer-side close - is free to tear it
/// down. `pea2pea` deliberately does not gate this on `on_connect` completion; once the
/// handshake is done, the connection belongs to the user.
///
/// In particular, **there is no guarantee that `on_connect` completes before
/// [`OnDisconnect::on_disconnect`] runs for the same address.** A short-lived peer that
/// immediately closes the socket after handshaking can produce that exact ordering.
///
/// Treat `on_connect` as a best-effort hook for quick, idempotent setup: populating an
/// `addr -> peer_id` map, sending a greeting, registering the peer with an external service,
/// etc. If your application needs a strict "setup-before-teardown" ordering, enforce it in
/// your own state - for example, mark the peer "ready" at the end of `on_connect` and have
/// [`OnDisconnect::on_disconnect`] skip (or defer) work for peers that were never marked ready.
pub trait OnConnect: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// Determines whether the [`OnConnect`] logic is guaranteed to run to completion.
    ///
    /// By default (`true`), if a disconnect happens before the associated task is first polled,
    /// it gets aborted and [`OnConnect::on_connect`] is silently skipped. This is rare in
    /// practice, but possible under heavy churn.
    ///
    /// Setting this to `false` detaches the task from the connection's lifecycle, so nothing
    /// can abort it before it runs - useful for bookkeeping that must pair with
    /// [`OnDisconnect::on_disconnect`] (counters, registries, audit logs, replication of peer
    /// state into an external store).
    ///
    /// In exchange, implementers must ensure that:
    /// - [`OnConnect::on_connect`] terminates in bounded time, since the library will no longer
    ///   cancel it on disconnect or shutdown
    /// - the logic tolerates the connection no longer being live by the time it actually runs,
    ///   in case a disconnect raced in first
    const ABORTABLE: bool = true;

    /// Attaches the behavior specified in [`OnConnect::on_connect`] right after every successful
    /// handshake.
    ///
    /// # Panics
    ///
    /// Panics if called more than once on the same [`Node`].
    fn enable_on_connect(&self) -> impl Future<Output = ()> {
        async {
            let (from_node_sender, from_node_receiver) =
                mpsc::channel::<((SocketAddr, u64), oneshot::Sender<OnConnectBundle>)>(
                    self.node().config().max_connections as usize,
                );

            // spawn a background task dedicated to executing the desired post-handshake actions
            let self_clone = self.clone();
            let handler_loop = async move {
                let node = self_clone.node().clone();
                run_hook_handler_loop(node, from_node_receiver, |((addr, conn_id), notifier)| {
                    let self_clone = self_clone.clone();
                    let handle = tokio::spawn(async move {
                        // disconnect automatically if the OnConnect impl panics
                        let mut conn_cleanup = DisconnectOnDrop::new(
                            self_clone.node().clone(),
                            addr,
                            conn_id,
                            DisconnectOrigin::OnConnectAbort,
                        );
                        // perform the specified initial actions
                        match AssertUnwindSafe(self_clone.on_connect(addr))
                            .catch_unwind()
                            .await
                        {
                            // if there was no panic, do not disconnect - this "defuses" the auto-cleanup
                            Ok(()) => {
                                conn_cleanup.node.take();
                            }
                            Err(payload) => {
                                let conn_span =
                                    create_connection_span(addr, self_clone.node().span());
                                error!(parent: conn_span, "OnConnect::on_connect panicked: {}", panic_message(&*payload));
                                resume_unwind(payload); // conn_cleanup stays armed, drops on unwind → disconnect
                            }
                        }
                    });
                    // notify the node that the initial actions have been scheduled
                    let _ = notifier.send((handle, Self::ABORTABLE)); // can't really fail
                })
                .await;
            };

            install_protocol_handler(
                self.node(),
                NodeTask::OnConnect,
                "OnConnect",
                |protocols| &protocols.on_connect,
                ProtocolHandler(from_node_sender),
                handler_loop,
            )
            .await;
        }
    }

    /// Any initial actions to be executed after the handshake is concluded; in order to be able to
    /// communicate with the peer in the usual manner (i.e. via [`Writing`]), only its [`SocketAddr`]
    /// (as opposed to the related [`Connection`] object) is provided as an argument.
    fn on_connect(&self, addr: SocketAddr) -> impl Future<Output = ()> + Send;
}
