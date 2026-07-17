use std::{
    future::Future,
    net::SocketAddr,
    panic::{AssertUnwindSafe, resume_unwind},
    time::Duration,
};

use futures_util::FutureExt;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::timeout,
};
use tracing::*;

#[cfg(doc)]
use crate::{
    Connection, Node,
    protocols::{OnConnect, Reading, Writing},
};
use crate::{
    Pea2Pea,
    connections::{DisconnectOrigin, create_connection_span},
    node::NodeTask,
    protocols::{ProtocolHandler, install_protocol_handler, panic_message, run_hook_handler_loop},
};

// The value returned to the node is a bit complex, so use an alias to break it down.
pub(crate) type OnDisconnectBundle = (JoinHandle<()>, oneshot::Receiver<()>);

/// Can be used to automatically perform some extra actions when the connection with a peer is
/// severed, which is especially practical if the disconnect is triggered automatically, e.g. due
/// to the peer sending a noncompliant message or when the peer is the one to shut down the
/// connection with the node.
///
/// note: The node can only tell that a peer disconnected from it if it is actively trying to read
/// from the associated connection (i.e. [`Reading`] is enabled) or if it attempts to send a message
/// to it (i.e. one of the [`Writing`] methods is called). This extends to idle or vanished peers:
/// the idle timeout is part of [`Reading`] ([`Reading::IDLE_TIMEOUT_MS`]), so without that protocol
/// a peer that goes away without a TCP FIN/RST holds its connection slot indefinitely.
///
/// note: This hook is executed before the connection is fully removed from the node's internal
/// state. Calls to [`Node::disconnect`] will wait for it to complete, ensuring that any necessary
/// cleanup (e.g., notifying a database) is finished before the function returns. The connection
/// remains live while the hook runs; in particular, [`Reading::process_message`] may still be
/// invoked for messages that arrive during it.
///
/// note: [`OnDisconnect::on_disconnect`] may run before [`OnConnect::on_connect`] for the same
/// address has finished, or even started - see [`OnConnect`] for details and recommended patterns.
pub trait OnDisconnect: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// The maximum time (in milliseconds) allowed for the [`OnDisconnect::on_disconnect`] hook to execute.
    /// If the hook exceeds this time, it will be aborted to ensure the node cleans up
    /// resources promptly.
    ///
    /// note: Unlike [`Reading::IDLE_TIMEOUT_MS`](Reading::IDLE_TIMEOUT_MS), a value of `0` does
    /// not disable the timeout - it aborts every hook (almost) instantly.
    const TIMEOUT_MS: u64 = 3_000;

    /// Attaches the behavior specified in [`OnDisconnect::on_disconnect`] to every occurrence of the
    /// node disconnecting from a peer.
    ///
    /// note: This hook is executed before the connection is fully removed from the node's internal
    /// state. Calls to [`Node::disconnect`] will wait for it to complete, ensuring that any
    /// necessary cleanup (e.g., notifying a database) is finished before the function returns.
    ///
    /// note: If the node has already begun shutting down, this is a no-op - the protocol is
    /// not enabled.
    ///
    /// # Panics
    ///
    /// Panics if called more than once on the same [`Node`].
    fn enable_on_disconnect(&self) -> impl Future<Output = ()> {
        async {
            let (from_node_sender, from_node_receiver) =
                mpsc::channel::<(
                    (SocketAddr, DisconnectOrigin),
                    oneshot::Sender<OnDisconnectBundle>,
                )>(self.node().config().max_connections as usize);

            // spawn a background task dedicated to handling disconnect events
            let self_clone = self.clone();
            let handler_loop = async move {
                let node = self_clone.node().clone();
                run_hook_handler_loop(node, from_node_receiver, |((addr, origin), notifier)| {
                    let self_clone = self_clone.clone();

                    // create a channel for waiting on completion
                    let (done_tx, done_rx) = oneshot::channel();

                    let handle = tokio::spawn(async move {
                        let hook = AssertUnwindSafe(self_clone.on_disconnect(addr, origin))
                            .catch_unwind();
                        // perform the specified extra actions
                        match timeout(Duration::from_millis(Self::TIMEOUT_MS), hook).await {
                            Ok(Ok(())) => {}
                            Ok(Err(payload)) => {
                                let conn_span =
                                    create_connection_span(addr, self_clone.node().span());
                                error!(parent: conn_span, "OnDisconnect::on_disconnect panicked: {}", panic_message(&*payload));
                                resume_unwind(payload);
                            }
                            Err(_) => {
                                let conn_span =
                                    create_connection_span(addr, self_clone.node().span());
                                warn!(parent: conn_span, "OnDisconnect logic timed out");
                            }
                        }
                        // notify on completion
                        let _ = done_tx.send(());
                    });
                    // provide the node with a handle to the scheduled task,
                    // and a receiver that will notify it of its completion
                    if let Err((handle, _)) = notifier.send((handle, done_rx)) {
                        // the disconnect that requested this hook was cancelled mid-await; abort
                        // the hook rather than let it run detached, where it would outlive the
                        // connection's removal (and potentially even the node's shutdown)
                        handle.abort();
                    }
                })
                .await;
            };

            install_protocol_handler(
                self.node(),
                NodeTask::OnDisconnect,
                "OnDisconnect",
                |protocols| &protocols.on_disconnect,
                ProtocolHandler(from_node_sender),
                handler_loop,
            )
            .await;
        }
    }

    /// Any extra actions to be executed during a disconnect; in order to still be able to
    /// communicate with the peer in the usual manner (i.e. via [`Writing`]), only its [`SocketAddr`]
    /// (as opposed to the related [`Connection`] object) is provided as an argument.
    ///
    /// note: To send a final message to the peer from this hook and be sure it is flushed before
    /// teardown, use [`Writing::unicast`] and await the returned receiver (within
    /// [`OnDisconnect::TIMEOUT_MS`]). [`Writing::unicast_fast`] and [`Writing::broadcast`] return no
    /// delivery feedback, so a message queued through them may not reach the socket before the
    /// writer is torn down. Sending a final message is not possible when the disconnect
    /// originates from the writer side itself ([`DisconnectOrigin::Writing`]): the writer is
    /// already defunct by the time the hook runs, and sends fail with `NotConnected`/`BrokenPipe`.
    fn on_disconnect(
        &self,
        addr: SocketAddr,
        origin: DisconnectOrigin,
    ) -> impl Future<Output = ()> + Send;
}
