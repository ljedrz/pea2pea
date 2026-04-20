use std::{future::Future, net::SocketAddr};

use tokio::sync::{mpsc, oneshot};
use tracing::*;

#[cfg(doc)]
use crate::{
    Connection, Node,
    protocols::{Handshake, OnDisconnect, Reading, Writing},
};
use crate::{
    Pea2Pea,
    node::NodeTask,
    protocols::{DisconnectOnDrop, ProtocolHandler},
};

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
/// `OnDisconnect::on_disconnect` skip (or defer) work for peers that were never marked ready.
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
                    self.node().config().max_connections as usize,
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
                        // disconnect automatically if the OnConnect impl panics
                        let mut conn_cleanup =
                            DisconnectOnDrop::new(self_clone2.node().clone(), addr);
                        // perform the specified initial actions
                        self_clone2.on_connect(addr).await;
                        // if there was no panic, do not disconnect - this "defuses" the auto-cleanup
                        conn_cleanup.node.take();
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
    fn on_connect(&self, addr: SocketAddr) -> impl Future<Output = ()> + Send;
}
