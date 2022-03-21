use crate::{
    protocols::{Protocol, ProtocolHandler, ReturnableConnection},
    Connection, Pea2Pea,
};

#[cfg(doc)]
use crate::{protocols::Handshake, Config, Node};

use async_trait::async_trait;
use futures_util::sink::SinkExt;
use parking_lot::RwLock;
use tokio::{
    io::AsyncWrite,
    net::tcp::OwnedWriteHalf,
    sync::{mpsc, oneshot},
};
use tokio_util::codec::{Encoder, FramedWrite};
use tracing::*;

use std::{any::Any, collections::HashMap, io, net::SocketAddr, sync::Arc};

/// Can be used to specify and enable writing, i.e. sending outbound messages. If the [`Handshake`]
/// protocol is enabled too, it goes into force only after the handshake has been concluded.
#[async_trait]
pub trait Writing: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// The type of the outbound messages; unless their serialization is expensive and the message
    /// is broadcasted (in which case it would get serialized multiple times), serialization should
    /// be done in the implementation of [`Self::Codec`].
    type Message: Send;

    /// The user-supplied [`Encoder`] used to write the outbound messages to the target stream.
    type Codec: Encoder<Self::Message, Error = io::Error> + Send;

    /// Prepares the node to send messages.
    async fn enable_writing(&self) {
        let (conn_sender, mut conn_receiver) = mpsc::unbounded_channel::<ReturnableConnection>();

        let conn_senders: Arc<RwLock<HashMap<SocketAddr, mpsc::Sender<WrappedMessage>>>> =
            Default::default();

        // Use a channel to know when the writing task is ready.
        let (tx_writing, rx_writing) = oneshot::channel::<()>();

        // the task spawning tasks reading messages from the given stream
        let self_clone = self.clone();
        let conn_senders_clone = conn_senders.clone();
        let writing_task = tokio::spawn(async move {
            trace!(parent: self_clone.node().span(), "spawned the Writing handler task");
            tx_writing.send(()).unwrap(); // safe; the channel was just opened

            // these objects are sent from `Node::adapt_stream`
            while let Some((mut conn, conn_returner)) = conn_receiver.recv().await {
                let addr = conn.addr();
                let codec = self_clone.codec(addr);
                let writer = self_clone.take_writer(&mut conn);
                let mut framed = FramedWrite::new(writer, codec);

                let (outbound_message_sender, mut outbound_message_receiver) =
                    mpsc::channel(self_clone.node().config().outbound_queue_depth);

                // register the connection's message sender with the Writing protocol handler
                conn_senders_clone
                    .write()
                    .insert(addr, outbound_message_sender);

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

                        match writer_clone.write_to_stream(*msg, &mut framed).await {
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
            handler: ProtocolHandler(conn_sender),
            senders: conn_senders,
        };
        assert!(
            self.node().protocols.writing_handler.set(hdl).is_ok(),
            "the Writing protocol was enabled more than once!"
        );
    }

    /// Creates an [`Encoder`] used to write the outbound messages to the target stream.
    fn codec(&self, _addr: SocketAddr) -> Self::Codec;

    /// Writes the given message to the network stream and returns the number of written bytes.
    async fn write_to_stream<W: AsyncWrite + Unpin + Send>(
        &self,
        message: Self::Message,
        writer: &mut FramedWrite<W, Self::Codec>,
    ) -> Result<usize, <Self::Codec as Encoder<Self::Message>>::Error> {
        writer.feed(message).await?;
        let len = writer.write_buffer().len();
        writer.flush().await?;

        Ok(len)
    }

    /// Sends the provided message to the specified [`SocketAddr`]. Returns as soon as the message is queued to
    /// be sent, without waiting for the actual delivery; instead, the caller is provided with a [`oneshot::Receiver`]
    /// which can be used to determine when and whether the message has been delivered.
    ///
    /// # Errors
    ///
    /// The following errors can be returned:
    /// - [`io::ErrorKind::NotConnected`] if the node is not connected to the provided address
    /// - [`io::ErrorKind::Other`] if the outbound message queue for this address is full
    /// - [`io::ErrorKind::Unsupported`] if [`Writing::enable_writing`] hadn't been called yet
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

    /// Broadcasts the provided message to all connected peers. Returns as soon as the message is queued to
    /// be sent to all the peers, without waiting for the actual delivery. This method doesn't provide the
    /// means to check when and if the messages actually get delivered; you can achieve that by calling
    /// [`Writing::send_direct_message`] for each address returned by [`Node::connected_addrs`].
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

    /// Obtains the write half of the connection to be used in [`Writing::enable_writing`].
    fn take_writer(&self, conn: &mut Connection) -> OwnedWriteHalf {
        conn.writer
            .take()
            .expect("Connection's writer is not available!")
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

/// The handler object dedicated to the [`Writing`] protocol.
pub struct WritingHandler {
    handler: ProtocolHandler<Connection, io::Result<Connection>>,
    pub(crate) senders: Arc<RwLock<HashMap<SocketAddr, mpsc::Sender<WrappedMessage>>>>,
}

impl Protocol<Connection, io::Result<Connection>> for WritingHandler {
    fn trigger(&self, item: ReturnableConnection) {
        self.handler.trigger(item);
    }
}
