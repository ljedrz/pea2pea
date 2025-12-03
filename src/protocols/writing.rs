use std::{
    any::Any, collections::HashMap, future::Future, io, net::SocketAddr, sync::Arc, time::Duration,
};

#[cfg(doc)]
use bytes::Bytes;
use futures_util::sink::SinkExt;
use parking_lot::RwLock;
use tokio::{
    io::AsyncWrite,
    sync::{mpsc, oneshot},
    task::JoinSet,
    time::timeout,
};
use tokio_util::codec::{Encoder, FramedWrite};
use tracing::*;

#[cfg(doc)]
use crate::{Config, Node, protocols::Handshake};
use crate::{
    Connection, ConnectionSide, Pea2Pea,
    node::NodeTask,
    protocols::{Protocol, ProtocolHandler, ReturnableConnection},
};

type WritingSenders = Arc<RwLock<HashMap<SocketAddr, mpsc::Sender<WrappedMessage>>>>;

/// Can be used to specify and enable writing, i.e. sending outbound messages. If the [`Handshake`]
/// protocol is enabled too, it goes into force only after the handshake has been concluded.
pub trait Writing: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// The depth of per-connection queues used to send outbound messages; the greater it is, the more outbound
    /// messages the node can enqueue. Setting it to a large value is not recommended, as doing it might
    /// obscure potential issues with your implementation (like slow serialization) or network.
    const MESSAGE_QUEUE_DEPTH: usize = 64;

    /// The initial size of a per-connection buffer for writing outbound messages. Can be set to the maximum expected size
    /// of the outbound message in order to only allocate it once.
    const INITIAL_BUFFER_SIZE: usize = 64 * 1024;

    // The maximum time (in milliseconds) allowed for a single message write to flush
    /// to the underlying stream before the connection is considered dead.
    const TIMEOUT_MS: u64 = 10_000;

    /// The type of the outbound messages; unless their serialization is expensive and the message
    /// is broadcasted (in which case it would get serialized multiple times), serialization should
    /// be done in the implementation of [`Self::Codec`].
    type Message: Send;

    /// The user-supplied [`Encoder`] used to write outbound messages to the target stream.
    type Codec: Encoder<Self::Message, Error = io::Error> + Send;

    /// Prepares the node to send messages.
    fn enable_writing(&self) -> impl Future<Output = ()> {
        async {
            // create a JoinSet to track all in-flight setup tasks
            let mut setup_tasks = JoinSet::new();

            let (conn_sender, mut conn_receiver) = mpsc::unbounded_channel();

            // the conn_senders are used to send messages from the Node to individual connections
            let conn_senders: WritingSenders = Default::default();
            // procure a clone to create the WritingHandler with
            let senders = conn_senders.clone();

            // use a channel to know when the writing task is ready
            let (tx_writing, rx_writing) = oneshot::channel();

            // the task spawning tasks sending messages to all the streams
            let self_clone = self.clone();
            let writing_task = tokio::spawn(async move {
                trace!(parent: self_clone.node().span(), "spawned the Writing handler task");
                if tx_writing.send(()).is_err() {
                    error!(parent: self_clone.node().span(), "writing handler creation interrupted! shutting down the node");
                    self_clone.node().shut_down().await;
                    return;
                }

                loop {
                    tokio::select! {
                        // handle new connections from `Node::adapt_stream`
                        maybe_conn = conn_receiver.recv() => {
                            match maybe_conn {
                                Some(returnable_conn) => {
                                    let self_clone2 = self_clone.clone();
                                    let senders = conn_senders.clone();
                                    setup_tasks.spawn(async move {
                                        self_clone2.handle_new_connection(returnable_conn, &senders).await;
                                    });
                                }
                                None => break, // channel closed
                            }
                        }
                        // task set cleanups
                        _ = setup_tasks.join_next(), if !setup_tasks.is_empty() => {}
                    }
                }
            });
            let _ = rx_writing.await;
            self.node()
                .tasks
                .lock()
                .insert(NodeTask::Writing, writing_task);

            // register the WritingHandler with the Node
            let hdl = WritingHandler {
                handler: ProtocolHandler(conn_sender),
                senders,
            };
            assert!(
                self.node().protocols.writing.set(hdl).is_ok(),
                "the Writing protocol was enabled more than once!"
            );
        }
    }

    /// Creates an [`Encoder`] used to write the outbound messages to the target stream.
    /// The `side` param indicates the connection side **from the node's perspective**.
    fn codec(&self, addr: SocketAddr, side: ConnectionSide) -> Self::Codec;

    /// Sends the provided message to the specified [`SocketAddr`]. Returns as soon as the message is queued to
    /// be sent, without waiting for the actual delivery; instead, the caller is provided with a [`oneshot::Receiver`]
    /// which can be used to determine when and whether the message has been delivered.
    ///
    /// # Errors
    ///
    /// The following errors can be returned:
    /// - [`io::ErrorKind::NotConnected`] if the node is not connected to the provided address
    /// - [`io::ErrorKind::QuotaExceeded`] if the outbound message queue for this address is full
    /// - [`io::ErrorKind::Unsupported`] if [`Writing::enable_writing`] hadn't been called yet
    fn unicast(
        &self,
        addr: SocketAddr,
        message: Self::Message,
    ) -> io::Result<oneshot::Receiver<io::Result<()>>> {
        // access the protocol handler
        if let Some(handler) = self.node().protocols.writing.get() {
            // find the message sender for the given address
            if let Some(sender) = handler.senders.read().get(&addr).cloned() {
                let (msg, delivery) = WrappedMessage::new(Box::new(message), true);
                sender
                    .try_send(msg)
                    .map_err(|e| {
                        error!(parent: self.node().span(), "can't send a message to {addr}: {e}");
                        io::ErrorKind::QuotaExceeded.into()
                    })
                    .map(|_| delivery.unwrap()) // infallible
            } else {
                Err(io::ErrorKind::NotConnected.into())
            }
        } else {
            Err(io::ErrorKind::Unsupported.into())
        }
    }

    /// Sends the provided message to the specified [`SocketAddr`], and returns as soon as the
    /// message is queued to be sent, without waiting for the actual delivery.
    ///
    /// # Errors
    ///
    /// See the error section for [`Writing::unicast`].
    fn unicast_fast(&self, addr: SocketAddr, message: Self::Message) -> io::Result<()> {
        // access the protocol handler
        if let Some(handler) = self.node().protocols.writing.get() {
            // find the message sender for the given address
            if let Some(sender) = handler.senders.read().get(&addr).cloned() {
                let (msg, _) = WrappedMessage::new(Box::new(message), false);
                sender.try_send(msg).map_err(|e| {
                    error!(parent: self.node().span(), "can't send a message to {addr}: {e}");
                    io::ErrorKind::QuotaExceeded.into()
                })
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
    /// [`Writing::unicast`] for each address returned by [`Node::connected_addrs`].
    ///
    /// note: This method clones the message for every connected peer, and serialization via
    /// [`Writing::Codec`] happens individually for each connection. If your serialization is
    /// expensive (e.g., large JSON/Bincode structs), manually serialize your message into
    /// [`Bytes`] *before* calling broadcast. This ensures serialization happens only once,
    /// rather than N times.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] if [`Writing::enable_writing`] hadn't been called yet.
    fn broadcast(&self, message: Self::Message) -> io::Result<()>
    where
        Self::Message: Clone,
    {
        // access the protocol handler
        if let Some(handler) = self.node().protocols.writing.get() {
            let senders = handler.senders.read().clone();
            for (addr, message_sender) in senders {
                let (msg, _) = WrappedMessage::new(Box::new(message.clone()), false);
                let _ = message_sender.try_send(msg).map_err(|e| {
                    error!(parent: self.node().span(), "can't send a message to {addr}: {e}");
                });
            }

            Ok(())
        } else {
            Err(io::ErrorKind::Unsupported.into())
        }
    }
}

/// This trait is used to restrict access to methods that would otherwise be public in [`Writing`].
trait WritingInternal: Writing {
    /// Writes the given message to the network stream and returns the number of written bytes.
    async fn write_to_stream<W: AsyncWrite + Unpin + Send>(
        &self,
        message: Self::Message,
        writer: &mut FramedWrite<W, Self::Codec>,
    ) -> Result<usize, <Self::Codec as Encoder<Self::Message>>::Error>;

    /// Applies the [`Writing`] protocol to a single connection.
    async fn handle_new_connection(
        &self,
        conn_with_returner: ReturnableConnection,
        conn_senders: &WritingSenders,
    );
}

impl<W: Writing> WritingInternal for W {
    async fn write_to_stream<A: AsyncWrite + Unpin + Send>(
        &self,
        message: Self::Message,
        writer: &mut FramedWrite<A, Self::Codec>,
    ) -> Result<usize, <Self::Codec as Encoder<Self::Message>>::Error> {
        writer.feed(message).await?;
        let len = writer.write_buffer().len();
        // guard against write starvation
        match timeout(Duration::from_millis(W::TIMEOUT_MS), writer.flush()).await {
            Ok(Ok(())) => Ok(len),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "write timed out")),
        }
    }

    async fn handle_new_connection(
        &self,
        (mut conn, conn_returner): ReturnableConnection,
        conn_senders: &WritingSenders,
    ) {
        let addr = conn.addr();
        let codec = self.codec(addr, !conn.side());
        let writer = conn.writer.take().expect("missing connection writer!");
        let mut framed = FramedWrite::new(writer, codec);

        if Self::INITIAL_BUFFER_SIZE != 0 {
            framed.write_buffer_mut().reserve(Self::INITIAL_BUFFER_SIZE);
        }

        let (outbound_message_sender, mut outbound_message_receiver) =
            mpsc::channel(Self::MESSAGE_QUEUE_DEPTH);

        // register the connection's message sender with the Writing protocol handler
        conn_senders.write().insert(addr, outbound_message_sender);

        // this will automatically drop the sender upon a disconnect
        let auto_cleanup = SenderCleanup {
            addr,
            senders: Arc::clone(conn_senders),
        };

        // use a channel to know when the writer task is ready
        let (tx_writer, rx_writer) = oneshot::channel();

        // the task for writing outbound messages
        let self_clone = self.clone();
        let conn_stats = conn.stats().clone();
        let writer_task = tokio::spawn(Box::pin(async move {
            let node = self_clone.node();
            trace!(parent: node.span(), "spawned a task for writing messages to {addr}");
            if tx_writer.send(()).is_err() {
                error!(parent: node.span(), "Writing for {addr} was interrupted; shutting down its task");
                return;
            }

            // move the cleanup into the task that gets aborted on disconnect
            let _auto_cleanup = auto_cleanup;

            while let Some(wrapped_msg) = outbound_message_receiver.recv().await {
                let msg = wrapped_msg.msg.downcast().unwrap();

                match self_clone.write_to_stream(*msg, &mut framed).await {
                    Ok(len) => {
                        if let Some(tx) = wrapped_msg.delivery_notification {
                            let _ = tx.send(Ok(()));
                        }
                        conn_stats.register_sent_message(len);
                        node.stats().register_sent_message(len);
                        trace!(parent: node.span(), "sent {len}B to {addr}");
                    }
                    Err(e) => {
                        error!(parent: node.span(), "couldn't send a message to {addr}: {e}");
                        if let Some(tx) = wrapped_msg.delivery_notification {
                            let _ = tx.send(Err(e));
                        }
                        break;
                    }
                }
            }

            let _ = node.disconnect(addr).await;
        }));
        let _ = rx_writer.await;
        conn.tasks.push(writer_task);

        // return the Connection to the Node, resuming Node::adapt_stream
        if conn_returner.send(Ok(conn)).is_err() {
            error!(parent: self.node().span(), "couldn't return a Connection with {addr} from the Writing handler");
        }
    }
}

/// Used to queue messages for delivery and return its confirmation.
pub(crate) struct WrappedMessage {
    msg: Box<dyn Any + Send>,
    delivery_notification: Option<oneshot::Sender<io::Result<()>>>,
}

impl WrappedMessage {
    fn new(
        msg: Box<dyn Any + Send>,
        confirmation: bool,
    ) -> (Self, Option<oneshot::Receiver<io::Result<()>>>) {
        let (tx, rx) = if confirmation {
            let (tx, rx) = oneshot::channel();
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };

        let wrapped_msg = Self {
            msg,
            delivery_notification: tx,
        };

        (wrapped_msg, rx)
    }
}

/// The handler object dedicated to the [`Writing`] protocol.
pub(crate) struct WritingHandler {
    handler: ProtocolHandler<Connection, io::Result<Connection>>,
    pub(crate) senders: WritingSenders,
}

impl Protocol<Connection, io::Result<Connection>> for WritingHandler {
    fn trigger(&self, item: ReturnableConnection) {
        self.handler.trigger(item);
    }
}

struct SenderCleanup {
    addr: SocketAddr,
    senders: WritingSenders,
}

impl Drop for SenderCleanup {
    fn drop(&mut self) {
        self.senders.write().remove(&self.addr);
    }
}
