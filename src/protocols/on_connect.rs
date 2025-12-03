use std::{future::Future, net::SocketAddr};

use tokio::sync::{mpsc, oneshot};
use tracing::*;

#[cfg(doc)]
use crate::{
    Connection, Node,
    protocols::{Handshake, OnDisconnect, Reading, Writing},
};
use crate::{Pea2Pea, node::NodeTask, protocols::ProtocolHandler};

/// Can be used to automatically perform some initial actions when a connection with a peer is
/// fully established. The reason for its existence (instead of including such behavior in the
/// [`Handshake`] impl) is that it allows the user to utilize [`Writing`] (if implemented and
/// enabled) and [`OnDisconnect`], while [`Handshake`] is very low-level and doesn't provide any
/// additional functionality - its purpose is only to establish a connection.
pub trait OnConnect: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// Attaches the behavior specified in [`OnConnect::on_connect`] right after every successful
    /// handshake.
    fn enable_on_connect(&self) -> impl Future<Output = ()> {
        async {
            let (from_node_sender, mut from_node_receiver) =
                mpsc::channel::<(SocketAddr, oneshot::Sender<tokio::task::JoinHandle<()>>)>(
                    self.node().config().max_connecting as usize,
                );

            // use a channel to know when the on_connect task is ready
            let (tx, rx) = oneshot::channel::<()>();

            // spawn a background task dedicated to executing the desired post-handshake actions
            let self_clone = self.clone();
            let on_connect_task = tokio::spawn(async move {
                trace!(parent: self_clone.node().span(), "spawned the OnConnect handler task");
                if tx.send(()).is_err() {
                    error!(parent: self_clone.node().span(), "OnConnect handler creation interrupted! shutting down the node");
                    self_clone.node().shut_down().await;
                    return;
                }

                while let Some((addr, notifier)) = from_node_receiver.recv().await {
                    let self_clone2 = self_clone.clone();
                    let handle = tokio::spawn(async move {
                        // perform the specified initial actions
                        self_clone2.on_connect(addr).await;
                    });
                    // notify the node that the initial actions have been scheduled
                    let _ = notifier.send(handle); // can't really fail
                }
            });
            let _ = rx.await;
            self.node()
                .tasks
                .lock()
                .insert(NodeTask::OnConnect, on_connect_task);

            // register the OnConnect handler with the Node
            let hdl = ProtocolHandler(from_node_sender);
            assert!(
                self.node().protocols.on_connect.set(hdl).is_ok(),
                "the OnConnect protocol was enabled more than once!"
            );
        }
    }

    /// Any initial actions to be executed after the handshake is concluded; in order to be able to
    /// communicate with the peer in the usual manner (i.e. via [`Writing`]), only its [`SocketAddr`]
    /// (as opposed to the related [`Connection`] object) is provided as an argument.
    ///
    /// note: If your implementation panics, the node will **not** automatically disconnect the
    /// peer - the connection will remain active in the node's state, potentially creating a
    /// "zombie" peer that is connected at the TCP level but uninitialized in your application
    /// logic.
    fn on_connect(&self, addr: SocketAddr) -> impl Future<Output = ()> + Send;
}
