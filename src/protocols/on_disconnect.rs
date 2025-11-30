use std::{future::Future, net::SocketAddr};

use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tracing::*;

#[cfg(doc)]
use crate::{
    Connection, Node,
    protocols::{Reading, Writing},
};
use crate::{Pea2Pea, node::NodeTask, protocols::ProtocolHandler};

/// Can be used to automatically perform some extra actions when the connection with a peer is
/// severed, which is especially practical if the disconnect is triggered automatically, e.g. due
/// to the peer sending a noncompliant message or when the peer is the one to shut down the
/// connection with the node.
///
/// note: the node can only tell that a peer disconnected from it if it is actively trying to read
/// from the associated connection (i.e. [`Reading`] is enabled) or if it attempts to send a message
/// to it (i.e. one of the [`Writing`] methods is called).
pub trait OnDisconnect: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// Attaches the behavior specified in [`OnDisconnect::on_disconnect`] to every occurrence of the
    /// node disconnecting from a peer.
    ///
    /// note: This hook is executed before the connection is fully removed from the node's internal
    /// state. Calls to [`Node::disconnect`] will wait for it to complete, ensuring that any
    /// necessary cleanup (e.g., notifying a database) is finished before the function returns.
    fn enable_on_disconnect(&self) -> impl Future<Output = ()> {
        async {
            let (from_node_sender, mut from_node_receiver) = mpsc::unbounded_channel::<(
                SocketAddr,
                oneshot::Sender<(JoinHandle<()>, oneshot::Receiver<()>)>,
            )>();

            // use a channel to know when the disconnect task is ready
            let (tx, rx) = oneshot::channel::<()>();

            // spawn a background task dedicated to handling disconnect events
            let self_clone = self.clone();
            let disconnect_task = tokio::spawn(async move {
                trace!(parent: self_clone.node().span(), "spawned the OnDisconnect handler task");
                if tx.send(()).is_err() {
                    error!(parent: self_clone.node().span(), "OnDisconnect handler creation interrupted! shutting down the node");
                    self_clone.node().shut_down().await;
                    return;
                }

                while let Some((addr, notifier)) = from_node_receiver.recv().await {
                    let self_clone2 = self_clone.clone();

                    // create a channel for waiting on completion
                    let (done_tx, done_rx) = oneshot::channel();

                    let handle = tokio::spawn(async move {
                        // perform the specified extra actions
                        self_clone2.on_disconnect(addr).await;
                        // notify on completion
                        let _ = done_tx.send(());
                    });
                    // provide the node with a handle to the scheduled task,
                    // and a receiver that will notify it of its completion
                    let _ = notifier.send((handle, done_rx)); // can't really fail
                }
            });
            let _ = rx.await;
            self.node()
                .tasks
                .lock()
                .insert(NodeTask::OnDisconnect, disconnect_task);

            // register the OnDisconnect handler with the Node
            let hdl = ProtocolHandler(from_node_sender);
            assert!(
                self.node().protocols.on_disconnect.set(hdl).is_ok(),
                "the OnDisconnect protocol was enabled more than once!"
            );
        }
    }

    /// Any extra actions to be executed during a disconnect; in order to still be able to
    /// communicate with the peer in the usual manner (i.e. via [`Writing`]), only its [`SocketAddr`]
    /// (as opposed to the related [`Connection`] object) is provided as an argument.
    fn on_disconnect(&self, addr: SocketAddr) -> impl Future<Output = ()> + Send;
}
