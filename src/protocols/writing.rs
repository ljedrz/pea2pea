use std::{
    any::Any,
    collections::{HashMap, hash_map::Entry},
    future::Future,
    io,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
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
    connections::create_connection_span,
    node::NodeTask,
    protocols::{
        DisconnectOnDrop, Protocol, ProtocolHandler, ReturnableConnection, log_setup_join,
    },
};

type WritingSenders = Arc<RwLock<HashMap<SocketAddr, (u64, Arc<dyn Any + Send + Sync>)>>>;

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

    /// The maximum time (in milliseconds) allowed for a single message write to flush
    /// to the underlying stream before the connection is considered dead.
    const TIMEOUT_MS: u64 = 10_000;

    /// The type of the outbound messages; unless their serialization is expensive and the message
    /// is broadcasted (in which case it would get serialized multiple times), serialization should
    /// be done in the implementation of [`Writing::Codec`].
    type Message: Send;

    /// The user-supplied [`Encoder`] used to write outbound messages to the target stream.
    type Codec: Encoder<Self::Message, Error = io::Error> + Send;

    /// Prepares the node to send messages.
    ///
    /// # Panics
    ///
    /// Panics if called more than once on the same [`Node`].
    fn enable_writing(&self) -> impl Future<Output = ()> {
        async {
            assert!(
                self.node().protocols.writing.get().is_none(),
                "the Writing protocol was enabled more than once!"
            );

            // create a JoinSet to track all in-flight setup tasks
            let mut setup_tasks = JoinSet::new();

            // a monotonic unique Sender ID generator
            let sender_id_generator: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));

            let (conn_sender, mut conn_receiver) =
                mpsc::channel(self.node().config().max_connecting as usize);

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
                                    let sender_id = sender_id_generator.fetch_add(1, Ordering::Relaxed);
                                    setup_tasks.spawn(async move {
                                        self_clone2.handle_new_connection(returnable_conn, &senders, sender_id).await;
                                    });
                                }
                                None => break, // channel closed
                            }
                        }
                        // task set cleanups
                        res = setup_tasks.join_next(), if !setup_tasks.is_empty() => {
                            log_setup_join(self_clone.node().span(), "Writing", res);
                        }
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
    /// - [`io::ErrorKind::BrokenPipe`] if the outbound message channel is down
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
            if let Some(erased) = handler.senders.read().get(&addr).map(|(_, s)| s.clone()) {
                let sender = erased
                    .downcast_ref::<mpsc::Sender<WrappedMessage<Self::Message>>>()
                    .unwrap(); // same Self => same M => TypeId always matches

                let (msg, delivery) = WrappedMessage::new(message, true);
                sender
                    .try_send(msg)
                    .map_err(|e| {
                        let conn_span = create_connection_span(addr, self.node().span());
                        error!(parent: conn_span, "can't send a message: {e}");
                        match e {
                            mpsc::error::TrySendError::Full(_) => {
                                io::ErrorKind::QuotaExceeded.into()
                            }
                            mpsc::error::TrySendError::Closed(_) => {
                                io::ErrorKind::BrokenPipe.into()
                            }
                        }
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
    /// message is queued to be sent, without waiting for the actual delivery (as opposed to
    /// [`Writing::unicast`], which does provide delivery feedback).
    ///
    /// # Errors
    ///
    /// See the error section for [`Writing::unicast`].
    fn unicast_fast(&self, addr: SocketAddr, message: Self::Message) -> io::Result<()> {
        // access the protocol handler
        if let Some(handler) = self.node().protocols.writing.get() {
            // find the message sender for the given address
            if let Some(erased) = handler.senders.read().get(&addr).map(|(_, s)| s.clone()) {
                let sender = erased
                    .downcast_ref::<mpsc::Sender<WrappedMessage<Self::Message>>>()
                    .unwrap(); // same Self => same M => TypeId always matches

                let (msg, _) = WrappedMessage::new(message, false);
                sender.try_send(msg).map_err(|e| {
                    let conn_span = create_connection_span(addr, self.node().span());
                    error!(parent: conn_span, "can't send a message: {e}");
                    match e {
                        mpsc::error::TrySendError::Full(_) => io::ErrorKind::QuotaExceeded.into(),
                        mpsc::error::TrySendError::Closed(_) => io::ErrorKind::BrokenPipe.into(),
                    }
                })
            } else {
                Err(io::ErrorKind::NotConnected.into())
            }
        } else {
            Err(io::ErrorKind::Unsupported.into())
        }
    }

    /// Broadcasts the provided message to all connected peers. Returns as soon as the message
    /// is queued to be sent to all the peers, without waiting for the actual delivery. For any
    /// peer whose queue is full or whose channel has been closed, the message is silently
    /// dropped and the failure is logged at `ERROR` level - the call still returns `Ok(())` for
    /// the broadcast as a whole and continues delivering to the remaining peers.
    ///
    /// If you need to know which peers received (or even queued) the message, call
    /// [`Writing::unicast`] for each address returned by [`Node::connected_addrs`] and inspect
    /// the returned [`oneshot::Receiver`]s.
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
            for (addr, erased) in senders {
                let sender = erased
                    .1
                    .downcast_ref::<mpsc::Sender<WrappedMessage<Self::Message>>>()
                    .unwrap(); // same Self => same M => TypeId always matches

                let (msg, _) = WrappedMessage::new(message.clone(), false);
                let _ = sender.try_send(msg).map_err(|e| {
                    let conn_span = create_connection_span(addr, self.node().span());
                    error!(parent: conn_span, "can't send a message: {e}");
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
    /// Feeds a whole batch of messages into the stream's buffer, flushes once,
    /// and returns the number of written messages and their cumulative size in bytes.
    async fn write_batch<W: AsyncWrite + Unpin + Send>(
        &self,
        messages: &mut Vec<WrappedMessage<Self::Message>>,
        writer: &mut FramedWrite<W, Self::Codec>,
    ) -> io::Result<(usize, usize)>;

    /// Applies the [`Writing`] protocol to a single connection.
    async fn handle_new_connection(
        &self,
        conn_with_returner: ReturnableConnection,
        conn_senders: &WritingSenders,
        sender_id: u64,
    );
}

impl<W: Writing> WritingInternal for W {
    async fn write_batch<A: AsyncWrite + Unpin + Send>(
        &self,
        messages: &mut Vec<WrappedMessage<Self::Message>>,
        writer: &mut FramedWrite<A, Self::Codec>,
    ) -> io::Result<(usize, usize)> {
        // Both the `feed`s and the final `flush` can block on a full socket send
        // buffer - `feed` flushes internally at the backpressure boundary - so
        // the entire sequence is guarded.
        let write = async move {
            let msgs = messages.len();
            let mut bytes = 0;

            let mut prev = writer.write_buffer().len();
            for wrapped in messages {
                let msg = wrapped.msg.take().unwrap(); // guaranteed to be present here
                writer.feed(msg).await?;
                let now = writer.write_buffer().len();
                bytes += now.checked_sub(prev).unwrap_or(now);
                prev = now;
            }
            writer.flush().await?;
            Ok::<_, io::Error>((msgs, bytes))
        };

        match timeout(Duration::from_millis(Self::TIMEOUT_MS), write).await {
            Ok(Ok(stats)) => Ok(stats),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "write timed out")),
        }
    }

    async fn handle_new_connection(
        &self,
        (mut conn, conn_returner): ReturnableConnection,
        conn_senders: &WritingSenders,
        sender_id: u64,
    ) {
        let addr = conn.addr();
        let codec = self.codec(addr, !conn.side());
        let Some(writer) = conn.writer.take() else {
            let err = io::Error::other("the stream was not returned during the handshake");
            error!(parent: conn.span(), "{err}");
            let _ = conn_returner.send(Err(err));
            return;
        };
        let mut framed = FramedWrite::new(writer, codec);

        if Self::INITIAL_BUFFER_SIZE != 0 {
            framed.write_buffer_mut().reserve(Self::INITIAL_BUFFER_SIZE);
        }

        let (outbound_message_sender, mut outbound_message_receiver) =
            mpsc::channel(Self::MESSAGE_QUEUE_DEPTH);

        // register the connection's message sender with the Writing protocol handler
        conn_senders.write().insert(
            addr,
            (
                sender_id,
                Arc::new(outbound_message_sender) as Arc<dyn Any + Send + Sync>,
            ),
        );

        // this will automatically drop the sender upon a disconnect
        let sender_cleanup = SenderCleanup {
            addr,
            sender_id,
            senders: Arc::clone(conn_senders),
        };

        // use a channel to know when the writer task is ready
        let (tx_writer, rx_writer) = oneshot::channel();

        // the task for writing outbound messages
        let self_clone = self.clone();
        let conn_stats = conn.stats().clone();
        let conn_span = conn.span().clone();
        let writer_task = tokio::spawn(Box::pin(async move {
            let node = self_clone.node();
            trace!(parent: &conn_span, "spawned a task for writing messages");
            if tx_writer.send(()).is_err() {
                error!(parent: &conn_span, "Writing was interrupted; shutting down its task");
                return;
            }

            // move the sender cleanup into ths task
            let _sender_cleanup = sender_cleanup;

            // disconnect automatically regardless of how this task concludes
            let _conn_cleanup = DisconnectOnDrop::new(node.clone(), addr);

            let mut batch: Vec<WrappedMessage<Self::Message>> = Vec::new();
            // recv_many blocks for the first message, then takes whatever else is
            // already queued (capped by the queue depth) - so we coalesce without
            // ever waiting to fill a batch. Returns 0 only once the channel closes.
            while outbound_message_receiver
                .recv_many(&mut batch, Self::MESSAGE_QUEUE_DEPTH)
                .await
                > 0
            {
                match self_clone.write_batch(&mut batch, &mut framed).await {
                    Ok((msgs, bytes)) => {
                        for tx in batch.drain(..).filter_map(|w| w.delivery_notification) {
                            let _ = tx.send(Ok(()));
                        }
                        conn_stats.register_sent_messages(msgs, bytes);
                        node.stats().register_sent_messages(msgs, bytes);
                        trace!(parent: &conn_span, "wrote {bytes}B ({msgs} messages)");
                    }
                    Err(e) => {
                        error!(parent: &conn_span, "couldn't write a batch of {} message(s): {e}", batch.len());
                        // the connection is going down; fail every queued delivery
                        for tx in batch.drain(..).filter_map(|w| w.delivery_notification) {
                            let _ = tx.send(Err(io::Error::new(e.kind(), e.to_string())));
                        }
                        break;
                    }
                }
            }
        }));
        let _ = rx_writer.await;
        conn.tasks.push(writer_task);

        // return the Connection to the Node, resuming Node::adapt_stream
        let conn_span = conn.span().clone();
        if conn_returner.send(Ok(conn)).is_err() {
            error!(parent: &conn_span, "couldn't return a Connection from the Writing handler");
        }
    }
}

/// Used to queue messages for delivery and return its confirmation.
pub(crate) struct WrappedMessage<T> {
    msg: Option<T>,
    delivery_notification: Option<oneshot::Sender<io::Result<()>>>,
}

impl<T> WrappedMessage<T> {
    fn new(msg: T, confirmation: bool) -> (Self, Option<oneshot::Receiver<io::Result<()>>>) {
        let (tx, rx) = if confirmation {
            let (tx, rx) = oneshot::channel();
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };

        let wrapped_msg = Self {
            msg: Some(msg),
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
    async fn trigger(&self, item: ReturnableConnection) {
        self.handler.trigger(item).await;
    }
}

struct SenderCleanup {
    addr: SocketAddr,
    senders: WritingSenders,
    sender_id: u64,
}

impl Drop for SenderCleanup {
    fn drop(&mut self) {
        let mut map = self.senders.write();
        if let Entry::Occupied(e) = map.entry(self.addr) {
            // only remove if this is still *our* sender; otherwise a newer
            // connection has reused the addr and we must not touch it
            if e.get().0 == self.sender_id {
                e.remove();
            }
        }
    }
}
