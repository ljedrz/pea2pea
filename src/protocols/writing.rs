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

                    while let Some(wrapped_msg) = outbound_message_receiver.recv().await {
                        let msg = wrapped_msg.msg.downcast::<Self::Message>().unwrap();

                        match writer_clone
                            .write_to_stream(*msg, addr, &mut buffer, &mut writer)
                            .await
                        {
                            Ok(len) => {
                                let _ = wrapped_msg.delivery_notification.send(true);
                                node.known_peers().register_sent_message(addr, len);
                                node.stats().register_sent_message(len);
                                trace!(parent: node.span(), "sent {}B to {}", len, addr);
                            }
                            Err(e) => {
                                let _ = wrapped_msg.delivery_notification.send(false);
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
    /// indicating its length, be suffixed with a character indicating that it's complete, etc. The `target`
    /// parameter is provided in case serialization depends on the recipient, e.g. in case of encryption.
    ///
    /// Note: the default `writer` is a memory buffer and thus writing to it is infallible.
    fn write_message<W: io::Write>(
        &self,
        target: SocketAddr,
        message: &Self::Message,
        writer: &mut W,
    ) -> io::Result<()>;

    /// Sends the provided message to the specified `SocketAddr`. Returns as soon as the message is queued to
    /// be sent, without waiting for the actual delivery; instead, the caller is provided with a [`oneshot::Receiver`]
    /// which can be used to determine when and whether the message has been delivered.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::NotConnected`] if the node is not connected to the provided address.
    ///
    /// Returns [`io::ErrorKind::Unsupported`] if [`Writing::enable_writing`] hadn't been called yet.
    fn send_direct_message(
        &self,
        addr: SocketAddr,
        message: Self::Message,
    ) -> io::Result<oneshot::Receiver<bool>> {
        // access the protocol handler
        if let Some(handler) = self.node().protocols.writing_handler.get() {
            // find the message sender for the given address
            if let Some(sender) = handler.senders.read().get(&addr).cloned() {
                let (msg, delivery) = WrappedMessage::new(Box::new(message));
                sender.try_send(msg).map_err(|e| {
                    error!(parent: self.node().span(), "can't send a message to {}: {}", addr, e);
                    self.node().stats().register_failure();
                    io::ErrorKind::Other.into()
                }).map(|_| delivery)
            } else {
                Err(io::ErrorKind::NotConnected.into())
            }
        } else {
            Err(io::ErrorKind::Unsupported.into())
        }
    }

    /// Broadcasts the provided message to all peers. Returns as soon as the message is queued to
    /// be sent to all the peers, without waiting for the actual delivery.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] if [`Writing::enable_writing`] hadn't been called yet.
    fn send_broadcast(&self, message: Self::Message) -> io::Result<()>
    where
        Self::Message: Clone,
    {
        // access the protocol handler
        if let Some(handler) = self.node().protocols.writing_handler.get() {
            let senders = handler.senders.read().clone();
            for (addr, message_sender) in senders {
                let (msg, _delivery) = WrappedMessage::new(Box::new(message.clone()));
                let _ = message_sender.try_send(msg).map_err(|e| {
                    error!(parent: self.node().span(), "can't send a message to {}: {}", addr, e);
                    self.node().stats().register_failure();
                });
            }

            Ok(())
        } else {
            Err(io::ErrorKind::Unsupported.into())
        }
    }
}

/// Used to queue messages for delivery.
pub(crate) struct WrappedMessage {
    msg: Box<dyn Any + Send>,
    delivery_notification: oneshot::Sender<bool>,
}

impl WrappedMessage {
    fn new(msg: Box<dyn Any + Send>) -> (Self, oneshot::Receiver<bool>) {
        let (tx, rx) = oneshot::channel();
        let wrapped_msg = Self {
            msg,
            delivery_notification: tx,
        };

        (wrapped_msg, rx)
    }
}

/// The handler object dedicated to the `Writing` protocol.
pub struct WritingHandler {
    handler: mpsc::Sender<ReturnableConnection>,
    pub(crate) senders: RwLock<HashMap<SocketAddr, mpsc::Sender<WrappedMessage>>>,
}

impl WritingHandler {
    pub(crate) async fn trigger(&self, item: ReturnableConnection) {
        if self.handler.send(item).await.is_err() {
            unreachable!(); // protocol's task is down! can't recover
        }
    }
}
