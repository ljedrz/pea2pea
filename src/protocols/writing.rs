use crate::{protocols::ReturnableConnection, Pea2Pea};

use async_trait::async_trait;
use parking_lot::RwLock;
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot},
};
use tracing::*;

use std::{any::Any, collections::HashMap, io, net::SocketAddr};

/// Can be used to specify and enable writing, i.e. sending outbound messages. If the `Handshake`
/// protocol is enabled too, it goes into force only after the handshake has been concluded.
#[async_trait]
pub trait Writing: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// The type of the outbound messages; unless their serialization is expensive and the message
    /// is broadcasted (in which case it would get serialized multiple times), serialization should
    /// be done in `Writing::write_message`.
    type Message: Send;

    /// Prepares the node to send messages.
    async fn enable_writing(&self) {
        let (conn_sender, mut conn_receiver) = mpsc::channel::<ReturnableConnection>(
            self.node().config().protocol_handler_queue_depth,
        );

        // Use a channel to know when the writing task is ready.
        let (tx_writing, rx_writing) = oneshot::channel::<()>();

        // the task spawning tasks reading messages from the given stream
        let self_clone = self.clone();
        let writing_task = tokio::spawn(async move {
            trace!(parent: self_clone.node().span(), "spawned the Writing handler task");
            tx_writing.send(()).unwrap(); // safe; the channel was just opened

            // these objects are sent from `Node::adapt_stream`
            while let Some((mut conn, conn_returner)) = conn_receiver.recv().await {
                let addr = conn.addr;
                let mut writer = conn.writer.take().unwrap(); // safe; it is available at this point
                let mut buffer = Vec::new();

                let (outbound_message_sender, mut outbound_message_receiver) =
                    mpsc::channel(self_clone.node().config().outbound_queue_depth);

                if let Some(handler) = self_clone.node().protocols.writing_handler.get() {
                    handler
                        .senders
                        .write()
                        .insert(addr, outbound_message_sender);
                } else {
                    unreachable!();
                }

                // Use a channel to know when the writer task is ready.
                let (tx_writer, rx_writer) = oneshot::channel::<()>();

                // the task for writing outbound messages
                let writer_clone = self_clone.clone();
                let writer_task = tokio::spawn(async move {
                    let node = writer_clone.node();
                    trace!(parent: node.span(), "spawned a task for writing messages to {}", addr);
                    tx_writer.send(()).unwrap(); // safe; the channel was just opened

                    // TODO: when try_recv is available in tokio again (https://github.com/tokio-rs/tokio/issues/3350),
                    // use try_recv() in order to write to the stream less often
                    while let Some(msg) = outbound_message_receiver.recv().await {
                        let msg = msg.downcast::<Self::Message>().unwrap();

                        match writer_clone
                            .write_to_stream(*msg, addr, &mut buffer, &mut writer)
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
                let _ = rx_writer.await;
                conn.tasks.push(writer_task);

                // return the Connection to the Node, resuming Node::adapt_stream
                if conn_returner.send(Ok(conn)).is_err() {
                    unreachable!("could't return a Connection to the Node");
                }
            }
        });
        let _ = rx_writing.await;
        self.node().tasks.lock().push(writing_task);

        // register the WritingHandler with the Node
        let hdl = WritingHandler {
            handler: conn_sender,
            senders: Default::default(),
        };
        assert!(
            self.node().protocols.writing_handler.set(hdl).is_ok(),
            "the Writing protocol was enabled more than once!"
        );
    }

    /// Writes the given message to the given writer, using the provided intermediate buffer; returns the number of
    /// bytes written to the writer.
    async fn write_to_stream<W: AsyncWrite + Unpin + Send>(
        &self,
        message: Self::Message,
        addr: SocketAddr,
        buffer: &mut Vec<u8>,
        writer: &mut W,
    ) -> io::Result<usize> {
        self.write_message(addr, &message, buffer)?;
        let len = buffer.len();
        writer.write_all(buffer).await?;
        buffer.clear();

        Ok(len)
    }

    /// Writes the provided payload to the given intermediate writer; the payload can get prepended with a header
    /// indicating its length, be suffixed with a character indicating that it's complete, etc.
    fn write_message<W: io::Write>(
        &self,
        target: SocketAddr,
        message: &Self::Message,
        writer: &mut W,
    ) -> io::Result<()>;

    /// Sends the provided message to the specified `SocketAddr`.
    fn send_direct_message(&self, addr: SocketAddr, message: Self::Message) -> io::Result<()> {
        if let Some(sender) = self
            .node()
            .protocols
            .writing_handler
            .get()
            .and_then(|h| h.senders.read().get(&addr).cloned())
        {
            sender.try_send(Box::new(message)).map_err(|e| {
                error!(parent: self.node().span(), "can't send a message to {}: {}", addr, e);
                self.node().stats().register_failure();
                io::ErrorKind::Other.into()
            })
        } else {
            Err(io::ErrorKind::Unsupported.into())
        }
    }

    /// Broadcasts the provided message to all peers.
    fn send_broadcast(&self, message: Self::Message)
    where
        Self::Message: Clone,
    {
        if let Some(handler) = self.node().protocols.writing_handler.get() {
            let senders = handler.senders.read().clone();
            for (addr, message_sender) in senders {
                let _ = message_sender.try_send(Box::new(message.clone())).map_err(|e| {
                    error!(parent: self.node().span(), "can't send a message to {}: {}", addr, e);
                    self.node().stats().register_failure();
                });
            }
        }
    }
}

/// The handler object dedicated to the `Writing` protocol.
pub struct WritingHandler {
    handler: mpsc::Sender<ReturnableConnection>,
    pub(crate) senders: RwLock<HashMap<SocketAddr, mpsc::Sender<Box<dyn Any + Send>>>>,
}

impl WritingHandler {
    pub(crate) async fn trigger(&self, item: ReturnableConnection) {
        if self.handler.send(item).await.is_err() {
            unreachable!(); // protocol's task is down! can't recover
        }
    }
}
