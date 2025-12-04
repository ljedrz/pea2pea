use std::{future::Future, io, net::SocketAddr, sync::Arc, time::Duration};

use bytes::BytesMut;
use futures_util::StreamExt;
use tokio::{
    io::AsyncRead,
    sync::{mpsc, oneshot},
    task::JoinSet,
    time::timeout,
};
use tokio_util::codec::{Decoder, FramedRead};
use tracing::*;

#[cfg(doc)]
use crate::{Config, protocols::Handshake};
use crate::{
    ConnectionInfo, ConnectionSide, Node, Pea2Pea, Stats,
    node::NodeTask,
    protocols::{DisconnectOnDrop, ProtocolHandler, ReturnableConnection},
};

/// Can be used to specify and enable reading, i.e. receiving inbound messages. If the [`Handshake`]
/// protocol is enabled too, it goes into force only after the handshake has been concluded.
///
/// Each inbound message is isolated by the user-supplied [`Reading::Codec`], creating a [`Reading::Message`],
/// which is immediately queued (with a [`Reading::MESSAGE_QUEUE_DEPTH`] limit) to be processed by
/// [`Reading::process_message`].
pub trait Reading: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// The depth of per-connection queues used to process inbound messages; the greater it is, the more inbound
    /// messages the node can enqueue, but setting it to a large value can make the node more susceptible to DoS
    /// attacks.
    const MESSAGE_QUEUE_DEPTH: usize = 64;

    /// Determines whether TCP backpressure should be exerted in case the number of queued messages
    /// reaches `MESSAGE_QUEUE_DEPTH`; if not, messages beyond the queue's capacity will be dropped.
    ///
    /// note: Setting this to `false` creates UDP-like behavior over TCP - the sender will receive
    /// no indication that their message was discarded. Only use this if your application protocol
    /// tolerates gaps in the data stream.
    const BACKPRESSURE: bool = true;

    /// The initial size of a per-connection buffer for reading inbound messages. Can be set to the
    /// maximum expected size of the inbound message in order to only allocate it once.
    ///
    /// note: This setting does **not** limit the maximum buffer growth. To prevent memory
    /// exhaustion attacks (where a peer sends a frame header declaring a massive size), you
    /// must enforce limits within your [`Self::Codec`] implementation.
    const INITIAL_BUFFER_SIZE: usize = 64 * 1024;

    /// The maximum time (in milliseconds) the node will wait for a new message
    /// before considering the connection dead. If it is set to `0`, there is no
    /// timeout.
    ///
    /// note: `pea2pea` does not enable TCP Keepalives on sockets by default. If you set this to
    /// `0` (disabled), a peer that loses power or has its cable pulled (without sending a TCP
    /// FIN/RST) will remain connected in your node's state indefinitely, consuming a connection
    /// slot.
    const IDLE_TIMEOUT_MS: u64 = 60_000;

    /// The final (deserialized) type of inbound messages.
    type Message: Send;

    /// The user-supplied [`Decoder`] used to interpret inbound messages.
    type Codec: Decoder<Item = Self::Message, Error = io::Error> + Send;

    /// Prepares the node to receive messages.
    fn enable_reading(&self) -> impl Future<Output = ()> {
        async {
            // create a JoinSet to track all in-flight setup tasks
            let mut setup_tasks = JoinSet::new();

            let (conn_sender, mut conn_receiver) =
                mpsc::channel(self.node().config().max_connecting as usize);

            // use a channel to know when the reading task is ready
            let (tx_reading, rx_reading) = oneshot::channel();

            // the main task spawning per-connection tasks reading messages from their streams
            let self_clone = self.clone();
            let reading_task = tokio::spawn(async move {
                trace!(parent: self_clone.node().span(), "spawned the Reading handler task");
                if tx_reading.send(()).is_err() {
                    error!(parent: self_clone.node().span(), "Reading handler creation interrupted! shutting down the node");
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
                                    setup_tasks.spawn(async move {
                                        self_clone2.handle_new_connection(returnable_conn).await;
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
            let _ = rx_reading.await;
            self.node()
                .tasks
                .lock()
                .insert(NodeTask::Reading, reading_task);

            // register the Reading handler with the Node
            let hdl = ProtocolHandler(conn_sender);
            assert!(
                self.node().protocols.reading.set(hdl).is_ok(),
                "the Reading protocol was enabled more than once!"
            );
        }
    }

    /// Creates a [`Decoder`] used to interpret messages from the network.
    /// The `side` param indicates the connection side **from the node's perspective**.
    fn codec(&self, addr: SocketAddr, side: ConnectionSide) -> Self::Codec;

    /// Processes an inbound message. Can be used to update state, send replies etc.
    ///
    /// note: This method is `await`ed sequentially in the connection's read loop. If it blocks or
    /// takes a long time to complete (e.g., database queries, heavy computation), the subsequent
    /// messages from this peer will be blocked. For any non-trivial work that doesn't need to be
    /// executed sequentially, use [`tokio::spawn`] to move processing to a background task to keep
    /// the connection loop responsive.
    fn process_message(
        &self,
        source: SocketAddr,
        message: Self::Message,
    ) -> impl Future<Output = ()> + Send;
}

/// This trait is used to restrict access to methods that would otherwise be public in [`Reading`].
trait ReadingInternal: Reading {
    /// Applies the [`Reading`] protocol to a single connection.
    fn handle_new_connection(
        &self,
        conn_with_returner: ReturnableConnection,
    ) -> impl Future<Output = ()> + Send;

    /// Wraps the user-supplied [`Decoder`] ([`Reading::Codec`]) in another one used for message accounting.
    fn map_codec<T: AsyncRead>(
        &self,
        framed: FramedRead<T, Self::Codec>,
        info: &ConnectionInfo,
    ) -> FramedRead<T, CountingCodec<Self::Codec>>;
}

impl<R: Reading> ReadingInternal for R {
    async fn handle_new_connection(&self, (mut conn, conn_returner): ReturnableConnection) {
        let addr = conn.addr();
        let codec = self.codec(addr, !conn.side());
        let Some(reader) = conn.reader.take() else {
            error!("The stream was not returned during the handshake with {addr}!");
            return;
        };
        let framed = FramedRead::with_capacity(reader, codec, Self::INITIAL_BUFFER_SIZE);
        let mut framed = self.map_codec(framed, conn.info());

        // the connection will notify the reading task once it's fully ready
        let (tx_conn_ready, rx_conn_ready) = oneshot::channel();
        conn.readiness_notifier = Some(tx_conn_ready);

        let (inbound_message_sender, mut inbound_message_receiver) =
            mpsc::channel(Self::MESSAGE_QUEUE_DEPTH);

        // use a channel to know when the processing task is ready
        let (tx_processing, rx_processing) = oneshot::channel::<()>();

        // the task for processing parsed messages
        let self_clone = self.clone();
        let inbound_processing_task = tokio::spawn(Box::pin(async move {
            let node = self_clone.node();
            trace!(parent: node.span(), "spawned a task for processing messages from {addr}");
            if tx_processing.send(()).is_err() {
                error!(parent: node.span(), "Reading (processing) for {addr} was interrupted; shutting down its task");
                return;
            }

            // disconnect automatically regardless of how this task concludes
            let _conn_cleanup = DisconnectOnDrop::new(node.clone(), addr);

            while let Some(msg) = inbound_message_receiver.recv().await {
                self_clone.process_message(addr, msg).await;
            }
        }));
        let _ = rx_processing.await;
        conn.tasks.push(inbound_processing_task);

        // use a channel to know when the reader task is ready
        let (tx_reader, rx_reader) = oneshot::channel::<()>();

        // the task for reading messages from a stream
        let node = self.node().clone();
        let reader_task = tokio::spawn(Box::pin(async move {
            trace!(parent: node.span(), "spawned a task for reading messages from {addr}");
            if tx_reader.send(()).is_err() {
                error!(parent: node.span(), "Reading (IO) for {addr} was interrupted; shutting down its task");
                return;
            }

            // postpone reads until the connection is fully established; if the process fails,
            // this task gets aborted, so there is no need for a dedicated timeout
            let _ = rx_conn_ready.await;

            // disconnect automatically regardless of how this task concludes
            let _conn_cleanup = DisconnectOnDrop::new(node.clone(), addr);

            loop {
                let next_frame_future = framed.next();

                let read_result = if Self::IDLE_TIMEOUT_MS != 0 {
                    match timeout(
                        Duration::from_millis(Self::IDLE_TIMEOUT_MS),
                        next_frame_future,
                    )
                    .await
                    {
                        Ok(res) => res, // IO completed (success or error)
                        Err(_) => {
                            debug!(parent: node.span(), "connection with {addr} timed out due to inactivity");
                            break;
                        }
                    }
                } else {
                    next_frame_future.await
                };

                match read_result {
                    Some(Ok(msg)) => {
                        // send the message for further processing
                        match Self::BACKPRESSURE {
                            true => {
                                if let Err(e) = inbound_message_sender.send(msg).await {
                                    error!(parent: node.span(), "can't process a message from {addr}: {e}");
                                    break;
                                }
                            }
                            false => {
                                if let Err(e) = inbound_message_sender.try_send(msg) {
                                    error!(parent: node.span(), "can't process a message from {addr}: {e}");
                                    if matches!(e, mpsc::error::TrySendError::Closed(_)) {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    Some(Err(e)) => {
                        error!(parent: node.span(), "can't read from {addr}: {e}");
                        // `tokio_util` codecs fuse on error, but break just to be safe
                        break;
                    }
                    None => break, // end of stream
                }
            }
        }));
        let _ = rx_reader.await;
        conn.tasks.push(reader_task);

        // return the Connection to the Node, resuming Node::adapt_stream
        if conn_returner.send(Ok(conn)).is_err() {
            error!(parent: self.node().span(), "couldn't return a Connection with {addr} from the Reading handler");
        }
    }

    fn map_codec<T: AsyncRead>(
        &self,
        framed: FramedRead<T, Self::Codec>,
        info: &ConnectionInfo,
    ) -> FramedRead<T, CountingCodec<Self::Codec>> {
        framed.map_decoder(|codec| CountingCodec {
            codec,
            node: self.node().clone(),
            addr: info.addr(),
            stats: info.stats().clone(),
            acc: 0,
        })
    }
}

/// A wrapper [`Decoder`] that also counts the inbound messages.
struct CountingCodec<D: Decoder> {
    codec: D,
    node: Node,
    addr: SocketAddr,
    stats: Arc<Stats>,
    acc: usize,
}

impl<D: Decoder> Decoder for CountingCodec<D> {
    type Item = D::Item;
    type Error = D::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let initial_buf_len = src.len();
        let ret = self.codec.decode(src)?;
        let final_buf_len = src.len();
        let read_len = initial_buf_len - final_buf_len + self.acc;

        if read_len != 0 {
            trace!(parent: self.node.span(), "read {read_len}B from {}", self.addr);

            if ret.is_some() {
                self.acc = 0;
                self.stats.register_received_message(read_len);
                self.node.stats().register_received_message(read_len);
            } else {
                self.acc = read_len;
            }
        }

        Ok(ret)
    }
}
