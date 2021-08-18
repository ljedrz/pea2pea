use crate::{protocols::ReturnableConnection, Pea2Pea};

use async_trait::async_trait;
use bytes::Bytes;
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::mpsc,
};
use tracing::*;

use std::{io, net::SocketAddr};

/// Can be used to specify and enable writing, i.e. sending outbound messages.
/// If handshaking is enabled too, it goes into force only after the handshake has been concluded.
#[async_trait]
pub trait Writing: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// Prepares the node to send messages.
    fn enable_writing(&self) {
        let (conn_sender, mut conn_receiver) = mpsc::channel::<ReturnableConnection>(
            self.node().config().protocol_handler_queue_depth,
        );

        // the task spawning tasks reading messages from the given stream
        let self_clone = self.clone();
        let writing_task = tokio::spawn(async move {
            trace!(parent: self_clone.node().span(), "spawned the Writing handler task");

            // these objects are sent from `Node::adapt_stream`
            while let Some((mut conn, conn_returner)) = conn_receiver.recv().await {
                let addr = conn.addr;
                let mut writer = conn.writer.take().unwrap(); // safe; it is available at this point
                let mut buffer = Vec::new();

                let (outbound_message_sender, mut outbound_message_receiver) =
                    mpsc::channel(self_clone.node().config().outbound_queue_depth);
                conn.outbound_message_sender = Some(outbound_message_sender);

                // the task for writing outbound messages
                let writer_clone = self_clone.clone();
                let writer_task = tokio::spawn(async move {
                    let node = writer_clone.node();
                    trace!(parent: node.span(), "spawned a task for writing messages to {}", addr);

                    // TODO: when try_recv is available in tokio again (https://github.com/tokio-rs/tokio/issues/3350),
                    // use try_recv() in order to write to the stream less often
                    while let Some(msg) = outbound_message_receiver.recv().await {
                        match writer_clone
                            .write_to_stream(&msg, addr, &mut buffer, &mut writer)
                            .await
                        {
                            Ok(len) => {
                                node.known_peers().register_sent_message(addr, len);
                                node.stats().register_sent_message(len);
                                trace!(parent: node.span(), "sent {}B to {}", len, addr);
                            }
                            Err(e) => {
                                node.known_peers().register_failure(addr);
                                error!(parent: node.span(), "couldn't send a message to {}: {}", addr, e);
                                if node.config().fatal_io_errors.contains(&e.kind()) {
                                    node.disconnect(addr).await;
                                    break;
                                }
                            }
                        }
                    }
                });
                conn.tasks.push(writer_task);

                // return the Connection to the Node, resuming Node::adapt_stream
                if conn_returner.send(Ok(conn)).is_err() {
                    unreachable!("could't return a Connection to the Node");
                }
            }
        });
        self.node().tasks.lock().push(writing_task);

        // register the WritingHandler with the Node
        self.node().set_writing_handler(conn_sender.into());
    }

    /// Writes the given message to the provided writer, using the provided intermediate buffer; returns the number of
    /// bytes written to the writer.
    async fn write_to_stream<W: AsyncWrite + Unpin + Send>(
        &self,
        message: &[u8],
        addr: SocketAddr,
        buffer: &mut Vec<u8>,
        writer: &mut W,
    ) -> io::Result<usize> {
        self.write_message(addr, message, buffer)?;
        let len = buffer.len();
        writer.write_all(buffer).await?;
        buffer.clear();

        Ok(len)
    }

    /// Writes the provided payload to the given intermediate writer; the payload can get prepended with a header
    /// indicating its length, be suffixed with a character indicating that it's complete, etc. Returns the number
    /// of bytes written to the writer.
    fn write_message<W: io::Write>(
        &self,
        target: SocketAddr,
        payload: &[u8],
        writer: &mut W,
    ) -> io::Result<()>;

    /// Sends the provided message to the specified `SocketAddr`.
    fn send_direct_message(&self, addr: SocketAddr, message: Bytes) -> io::Result<()> {
        self.node()
            .connections()
            .sender(addr)?
            .try_send(message)
            .map_err(|e| {
                error!(parent: self.node().span(), "can't send a message to {}: {}", addr, e);
                self.node().stats().register_failure();
                io::ErrorKind::Other.into()
            })
    }

    /// Broadcasts the provided message to all peers.
    fn send_broadcast(&self, message: Bytes) {
        for (message_sender, addr) in self.node().connections().senders() {
            let _ = message_sender.try_send(message.clone()).map_err(|e| {
                error!(parent: self.node().span(), "can't send a message to {}: {}", addr, e);
                self.node().stats().register_failure();
            });
        }
    }
}
