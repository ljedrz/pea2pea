use crate::{protocols::ReturnableConnection, Connection, Pea2Pea};

use tokio::{
    sync::{mpsc, oneshot},
    task,
    time::timeout,
};
use tracing::*;

use std::{io, time::Duration};

/// Can be used to specify and enable network handshakes. Upon establishing a connection, both sides will
/// need to adhere to the specified handshake rules in order to finalize the connection and be able to send
/// or receive any messages.
#[async_trait::async_trait]
pub trait Handshake: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// Prepares the node to perform specified network handshakes.
    async fn enable_handshake(&self) {
        let (from_node_sender, mut from_node_receiver) = mpsc::channel::<ReturnableConnection>(
            self.node().config().protocol_handler_queue_depth,
        );

        // Use a channel to know when the handshake task is ready.
        let (tx, rx) = oneshot::channel::<()>();

        // spawn a background task dedicated to handling the handshakes
        let self_clone = self.clone();
        let handshake_task = tokio::spawn(async move {
            trace!(parent: self_clone.node().span(), "spawned the Handshake handler task");
            tx.send(()).unwrap(); // safe; the channel was just opened

            while let Some((conn, result_sender)) = from_node_receiver.recv().await {
                let addr = conn.addr;

                let node = self_clone.clone();
                task::spawn(async move {
                    debug!(parent: node.node().span(), "shaking hands with {} as the {:?}", addr, !conn.side);
                    let result = timeout(
                        Duration::from_millis(node.node().config().max_handshake_time_ms),
                        node.perform_handshake(conn),
                    )
                    .await;

                    let ret = match result {
                        Ok(Ok(conn)) => {
                            debug!(parent: node.node().span(), "successfully handshaken with {}", addr);
                            Ok(conn)
                        }
                        Ok(Err(e)) => {
                            error!(parent: node.node().span(), "handshake with {} failed: {}", addr, e);
                            Err(e)
                        }
                        Err(_) => {
                            error!(parent: node.node().span(), "handshake with {} timed out", addr);
                            Err(io::ErrorKind::TimedOut.into())
                        }
                    };

                    // return the Connection to the Node, resuming Node::adapt_stream
                    if result_sender.send(ret).is_err() {
                        unreachable!("could't return a Connection to the Node");
                    }
                });
            }
        });
        let _ = rx.await;
        self.node().tasks.lock().push(handshake_task);

        // register the HandshakeHandler with the Node
        let hdl = HandshakeHandler(from_node_sender);
        assert!(
            self.node().protocols.handshake_handler.set(hdl).is_ok(),
            "the Handshake protocol was enabled more than once!"
        );
    }

    /// Performs the handshake; temporarily assumes control of the `Connection` and returns it if the handshake is
    /// successful.
    async fn perform_handshake(&self, conn: Connection) -> io::Result<Connection>;
}

/// The handler object dedicated to the `Handshake` protocol.
pub struct HandshakeHandler(mpsc::Sender<ReturnableConnection>);

impl HandshakeHandler {
    pub(crate) async fn trigger(&self, item: ReturnableConnection) {
        if self.0.send(item).await.is_err() {
            unreachable!(); // protocol's task is down! can't recover
        }
    }
}
