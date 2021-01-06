use crate::{connections::ConnectionReader, protocols::ReturnableConnection, Pea2Pea};

use async_trait::async_trait;
use tokio::{io::AsyncReadExt, sync::mpsc, task::JoinHandle, time::sleep};
use tracing::*;

use std::{io, net::SocketAddr, time::Duration};

/// Can be used to specify and enable reading, i.e. receiving inbound messages.
/// If handshaking is enabled too, it goes into force only after the handshake has been concluded.
#[async_trait]
pub trait Reading: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// The final (deserialized) type of incoming messages.
    type Message: Send;

    // TODO: add an associated type defaulting to ConnectionReader once
    // https://github.com/rust-lang/rust/issues/29661 is resolved.

    /// Prepares the node to receive messages; failures to read from a connection's stream are penalized by a timeout
    /// defined in `NodeConfig`, while broken/unreadable messages result in an immediate disconnect (in order to avoid
    /// accidentally reading "borked" messages).
    fn enable_reading(&self) {
        let (conn_sender, mut conn_receiver) =
            mpsc::channel::<ReturnableConnection>(self.node().config.reading_handler_queue_depth);

        // the main task spawning per-connection tasks reading messages from their streams
        let self_clone = self.clone();
        let reading_task = tokio::spawn(async move {
            trace!(parent: self_clone.node().span(), "spawned the Reading handler task");

            loop {
                // these objects are sent from `Node::adapt_stream`
                if let Some((mut conn, conn_returner)) = conn_receiver.recv().await {
                    let addr = conn.addr;
                    let mut conn_reader = conn.reader.take().unwrap(); // safe; it is available at this point

                    let (inbound_message_sender, mut inbound_message_receiver) =
                        mpsc::channel(self_clone.node().config.conn_inbound_queue_depth);

                    // the task for reading messages from a stream
                    let reader_clone = self_clone.clone();
                    let reader_task = tokio::spawn(async move {
                        let node = reader_clone.node();
                        trace!(parent: node.span(), "spawned a task for reading messages from {}", addr);

                        loop {
                            if let Err(e) = reader_clone
                                .read_from_stream(&mut conn_reader, &inbound_message_sender)
                                .await
                            {
                                node.known_peers().register_failure(addr);
                                match e.kind() {
                                    io::ErrorKind::InvalidData => {
                                        // drop the connection to avoid reading borked messages
                                        node.disconnect(addr);
                                        return;
                                    }
                                    io::ErrorKind::Other => {
                                        // an unsuccessful read from the stream is not fatal; instead of disconnecting,
                                        // impose a timeout before attempting another read
                                        sleep(Duration::from_secs(
                                            node.config.invalid_message_penalty_secs,
                                        ))
                                        .await;
                                    }
                                    _ => unreachable!(),
                                }
                            }
                        }
                    });

                    // the task for processing parsed messages
                    let processing_clone = self_clone.clone();
                    let inbound_processing_task = tokio::spawn(async move {
                        let node = processing_clone.node();
                        trace!(parent: node.span(), "spawned a task for processing messages from {}", addr);

                        loop {
                            if let Some(msg) = inbound_message_receiver.recv().await {
                                if let Err(e) = processing_clone.process_message(addr, msg).await {
                                    error!(parent: node.span(), "can't process an inbound message: {}", e);
                                    node.known_peers().register_failure(addr);
                                }
                            }
                        }
                    });

                    // the Connection object registers the handles of the
                    // newly created tasks before being returned to the Node
                    conn.tasks.push(reader_task);
                    conn.tasks.push(inbound_processing_task);

                    // return the Connection to the Node, resuming Node::adapt_stream
                    if conn_returner.send(Ok(conn)).is_err() {
                        // can't recover if this happens
                        panic!("can't return a Connection to the Node");
                    }
                }
            }
        });

        // register the ReadingHandler with the Node
        self.node()
            .set_reading_handler((conn_sender, reading_task).into());
    }

    /// Performs a read from the stream. The default implementation is buffered; it sacrifices a bit of simplicity for
    /// better performance. Read messages are sent to a message processing task, in order to allow faster reads from
    /// the stream.
    async fn read_from_stream(
        &self,
        conn_reader: &mut ConnectionReader,
        message_sender: &mpsc::Sender<Self::Message>,
    ) -> io::Result<()> {
        let ConnectionReader {
            node,
            addr,
            reader,
            buffer,
            carry,
        } = conn_reader;

        // perform a read from the stream, being careful not to overwrite any bytes carried over from the previous read
        match reader.read(&mut buffer[*carry..]).await {
            Ok(n) => {
                trace!(parent: node.span(), "read {}B from {}", n, addr);
                let mut processed = 0;
                let mut left = *carry + n;

                // several messages could have been read at once; process the contents of the buffer
                loop {
                    // try to read a single message from the buffer
                    match self.read_message(*addr, &buffer[processed..processed + left]) {
                        // a full message was read successfully
                        Ok(Some((msg, len))) => {
                            // advance the counters
                            processed += len;
                            left -= len;

                            trace!(
                                parent: node.span(),
                                "isolated {}B as a message from {}; {}B left to process",
                                len,
                                addr,
                                left
                            );
                            node.known_peers().register_received_message(*addr, len);
                            node.stats.register_received_message(len);

                            // send the message for further processing
                            if message_sender.send(msg).await.is_err() {
                                // can't recover from an error here
                                panic!("the inbound message channel is closed");
                            }

                            // if the read is exhausted, reset the carry and return
                            if left == 0 {
                                *carry = 0;
                                return Ok(());
                            }
                        }
                        // the message in the buffer is incomplete
                        Ok(None) => {
                            // forbid messages that are larger than the read buffer
                            if left >= buffer.len() {
                                error!(parent: node.span(), "a message from {} is too large", addr);
                                return Err(io::ErrorKind::InvalidData.into());
                            }

                            trace!(
                                parent: node.span(),
                                "a message from {} is incomplete; carrying {}B over",
                                addr,
                                left
                            );
                            *carry = left;

                            // move the leftover bytes to the beginning of the buffer; the next read will append bytes
                            // starting from where the leftover ones end, allowing the message to be completed
                            buffer.copy_within(processed..processed + left, 0);

                            return Ok(());
                        }
                        // an erroneous message (e.g. an unexpected zero-length payload)
                        Err(_) => {
                            error!(parent: node.span(), "a message from {} is invalid", addr);
                            return Err(io::ErrorKind::InvalidData.into());
                        }
                    }
                }
            }
            // a stream read error
            Err(e) => {
                error!(parent: node.span(), "can't read from {}: {}", addr, e);
                Err(io::ErrorKind::Other.into())
            }
        }
    }

    /// Reads a single message from `ConnectionReader`'s buffer; `Ok(None)` indicates that the message is
    /// incomplete, i.e. further reads from the stream must be performed in order to produce the whole message.
    /// Alongside the message it returns the number of bytes the read message occupied in the buffer. An `Err`
    /// returned here will result in the associated connection being dropped.
    fn read_message(
        &self,
        source: SocketAddr,
        buffer: &[u8],
    ) -> io::Result<Option<(Self::Message, usize)>>;

    /// Processes an inbound message. Can be used to update state, send replies etc.
    #[allow(unused_variables)]
    async fn process_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()> {
        // don't do anything by default
        Ok(())
    }
}

/// An object dedicated to spawning tasks handling inbound messages
/// from new connections; used in the `Reading` protocol.
pub struct ReadingHandler {
    sender: mpsc::Sender<ReturnableConnection>,
    _task: JoinHandle<()>,
}

impl ReadingHandler {
    /// Sends a returnable `Connection` to a task spawned by the `ReadingHandler`.
    pub async fn send(&self, returnable_conn: ReturnableConnection) {
        if self.sender.send(returnable_conn).await.is_err() {
            // can't recover if this happens
            panic!("ReadingHandler's Receiver is closed")
        }
    }
}

impl From<(mpsc::Sender<ReturnableConnection>, JoinHandle<()>)> for ReadingHandler {
    fn from((sender, _task): (mpsc::Sender<ReturnableConnection>, JoinHandle<()>)) -> Self {
        Self { sender, _task }
    }
}
