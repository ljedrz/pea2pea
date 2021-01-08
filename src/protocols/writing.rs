use crate::{connections::ConnectionWriter, protocols::ReturnableConnection, Pea2Pea};

use async_trait::async_trait;
use bytes::Bytes;
use tokio::{io::AsyncWriteExt, sync::mpsc};
use tracing::*;

use std::{io, net::SocketAddr};

/// Can be used to specify and enable writing, i.e. sending outbound messages.
/// If handshaking is enabled too, it goes into force only after the handshake has been concluded.
#[async_trait]
pub trait Writing: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    // TODO: add an associated type defaulting to ConnectionWriter once
    // https://github.com/rust-lang/rust/issues/29661 is resolved.

    /// Prepares the node to send messages.
    fn enable_writing(&self) {
        let (conn_sender, mut conn_receiver) = mpsc::channel::<ReturnableConnection>(
            self.node().config().protocol_handler_queue_depth,
        );

        // the task spawning tasks reading messages from the given stream
        let self_clone = self.clone();
        let writing_task = tokio::spawn(async move {
            trace!(parent: self_clone.node().span(), "spawned the Writing handler task");

            loop {
                // these objects are sent from `Node::adapt_stream`
                if let Some((mut conn, conn_returner)) = conn_receiver.recv().await {
                    let addr = conn.addr;
                    let mut conn_writer = conn.writer.take().unwrap(); // safe; it is available at this point

                    let (outbound_message_sender, mut outbound_message_receiver) =
                        mpsc::channel::<Bytes>(
                            self_clone.node().config().conn_outbound_queue_depth,
                        );

                    // the task for writing outbound messages
                    let writer_clone = self_clone.clone();
                    let writer_task = tokio::spawn(async move {
                        let node = writer_clone.node();
                        trace!(parent: node.span(), "spawned a task for writing messages to {}", addr);

                        loop {
                            // TODO: when try_recv is available in tokio again (https://github.com/tokio-rs/tokio/issues/3350),
                            // use try_recv() in order to write to the stream less often
                            if let Some(msg) = outbound_message_receiver.recv().await {
                                match writer_clone.write_to_stream(&msg, &mut conn_writer).await {
                                    Ok(len) => {
                                        node.known_peers().register_sent_message(addr, len);
                                        node.stats().register_sent_message(len);
                                        trace!(parent: node.span(), "sent {}B to {}", len, addr);
                                    }
                                    Err(e) => {
                                        node.known_peers().register_failure(addr);
                                        error!(parent: node.span(), "couldn't send a message to {}: {}", addr, e);
                                    }
                                }
                            } else {
                                node.disconnect(addr);
                                break;
                            }
                        }
                    });

                    // the Connection object registers the handle of the newly created task and
                    // the Sender that will allow the Node to transmit messages to it
                    conn.tasks.push(writer_task);
                    conn.outbound_message_sender = Some(outbound_message_sender);

                    // return the Connection to the Node, resuming Node::adapt_stream
                    if conn_returner.send(Ok(conn)).is_err() {
                        // can't recover if this happens
                        panic!("can't return a Connection to the Node");
                    }
                } else {
                    error!("the Writing protocol is down!");
                    break;
                }
            }
        });

        // register the WritingHandler with the Node
        self.node()
            .set_writing_handler((conn_sender, writing_task).into());
    }

    /// Writes the given message to `ConnectionWriter`'s stream; returns the number of bytes written.
    async fn write_to_stream(
        &self,
        message: &[u8],
        conn_writer: &mut ConnectionWriter,
    ) -> io::Result<usize> {
        let ConnectionWriter {
            span: _,
            addr,
            writer,
            buffer,
            carry: _,
        } = conn_writer;

        let len = self.write_message(*addr, message, buffer)?;
        writer.write_all(&buffer[..len]).await?;

        Ok(len)
    }

    /// Writes the provided payload to `ConnectionWriter`'s buffer; the payload can get prepended with a header
    /// indicating its length, be suffixed with a character indicating that it's complete, etc. Returns the number
    /// of bytes written to the buffer.
    fn write_message(
        &self,
        target: SocketAddr,
        payload: &[u8],
        buffer: &mut [u8],
    ) -> io::Result<usize>;
}
