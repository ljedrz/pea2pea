use std::net::SocketAddr;

use tokio::sync::{mpsc, oneshot};
use tracing::*;

use crate::{protocols::ProtocolHandler, Pea2Pea};
#[cfg(doc)]
use crate::{protocols::Writing, Connection};

/// Can be used to automatically perform some extra actions when the node disconnects from its
/// peer, which is especially practical if the disconnect is triggered automatically, e.g. due
/// to the peer exceeding the allowed number of failures or severing its connection with the node
/// on its own.
#[async_trait::async_trait]
pub trait Disconnect: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// Attaches the behavior specified in [`Disconnect::handle_disconnect`] to every occurrence of the
    /// node disconnecting from a peer.
    async fn enable_disconnect(&self) {
        let (from_node_sender, mut from_node_receiver) =
            mpsc::unbounded_channel::<(SocketAddr, oneshot::Sender<()>)>();

        // use a channel to know when the disconnect task is ready
        let (tx, rx) = oneshot::channel::<()>();

        // spawn a background task dedicated to handling disconnect events
        let self_clone = self.clone();
        let disconnect_task = tokio::spawn(async move {
            trace!(parent: self_clone.node().span(), "spawned the Disconnect handler task");
            tx.send(()).unwrap(); // safe; the channel was just opened

            while let Some((addr, notifier)) = from_node_receiver.recv().await {
                let self_clone2 = self_clone.clone();
                tokio::spawn(async move {
                    // perform the specified extra actions
                    self_clone2.handle_disconnect(addr).await;
                    // notify the node that the extra actions have concluded
                    // and that the related connection can be dropped
                    let _ = notifier.send(()); // can't really fail
                });
            }
        });
        let _ = rx.await;
        self.node().tasks.lock().push(disconnect_task);

        // register the Disconnect handler with the Node
        let hdl = Box::new(ProtocolHandler(from_node_sender));
        assert!(
            self.node().protocols.disconnect.set(hdl).is_ok(),
            "the Disconnect protocol was enabled more than once!"
        );
    }

    /// Any extra actions to be executed during a disconnect; in order to still be able to
    /// communicate with the peer in the usual manner (i.e. via [`Writing`]), only its [`SocketAddr`]
    /// (as opposed to the related [`Connection`] object) is provided as an argument.
    async fn handle_disconnect(&self, addr: SocketAddr);
}
