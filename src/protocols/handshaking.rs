use crate::{connections::Connection, protocols::ReturnableConnection, Pea2Pea};

use tokio::{sync::mpsc, time::timeout};
use tracing::*;

use std::{io, time::Duration};

/// Can be used to specify and enable network handshakes. Upon establishing a connection, both sides will
/// need to adhere to the specified handshake rules in order to finalize the connection and be able to send
/// or receive any messages.
#[async_trait::async_trait]
pub trait Handshaking: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// Prepares the node to perform specified network handshakes.
    fn enable_handshaking(&self) {
        let (from_node_sender, mut from_node_receiver) = mpsc::channel::<ReturnableConnection>(
            self.node().config().protocol_handler_queue_depth,
        );

        // spawn a background task dedicated to handling the handshakes
        let self_clone = self.clone();
        let handshaking_task = tokio::spawn(async move {
            trace!(parent: self_clone.node().span(), "spawned the Handshaking handler task");

            loop {
                if let Some((conn, result_sender)) = from_node_receiver.recv().await {
                    let addr = conn.addr;

                    debug!(parent: conn.node.span(), "handshaking with {} as the {:?}", addr, !conn.side);
                    let result = timeout(
                        Duration::from_millis(conn.node.config().max_handshake_time_ms),
                        self_clone.perform_handshake(conn),
                    )
                    .await;

                    let ret = match result {
                        Ok(Ok(res)) => {
                            debug!(parent: self_clone.node().span(), "succeessfully handshaken with {}", addr);
                            Ok(res)
                        }
                        Ok(Err(e)) => {
                            error!(parent: self_clone.node().span(), "handshake with {} failed: {}", addr, e);
                            Err(e)
                        }
                        Err(_) => {
                            error!(parent: self_clone.node().span(), "handshake with {} timed out", addr);
                            Err(io::ErrorKind::TimedOut.into())
                        }
                    };

                    // return the Connection to the Node, resuming Node::adapt_stream
                    if result_sender.send(ret).is_err() {
                        unreachable!(); // can't recover if this happens
                    }
                }
            }
        });
        self.node().tasks.lock().push(handshaking_task);

        self.node().set_handshake_handler(from_node_sender.into());
    }

    /// Performs the handshake; temporarily assumes control of the `Connection` and returns it if the handshake is
    /// successful.
    async fn perform_handshake(&self, conn: Connection) -> io::Result<Connection>;
}
