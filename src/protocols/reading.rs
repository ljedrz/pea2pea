use crate::{protocols::ReturnableConnection, Pea2Pea};

use async_trait::async_trait;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::{mpsc, oneshot},
    time::sleep,
};
use tracing::*;

use std::{io, net::SocketAddr, time::Duration};

/// Can be used to specify and enable reading, i.e. receiving inbound messages.
/// If `Handshake` is enabled too, it goes into force only after the handshake has been concluded.
#[async_trait]
pub trait Reading: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// The final (deserialized) type of inbound messages.
    type Message: Send;

    /// Prepares the node to receive messages; failures to read from a connection's stream are penalized by a timeout
    /// defined in `Config`, while the configured fatal errors result in an immediate disconnect (in order to e.g. avoid
    /// accidentally reading "borked" messages).
    async fn enable_reading(&self) {
        let (conn_sender, mut conn_receiver) = mpsc::channel::<ReturnableConnection>(
            self.node().config().protocol_handler_queue_depth,
        );

        // Use a channel to know when the reading task is ready.
        let (tx_reading, rx_reading) = oneshot::channel::<()>();

        // the main task spawning per-connection tasks reading messages from their streams
        let self_clone = self.clone();
        let reading_task = tokio::spawn(async move {
            trace!(parent: self_clone.node().span(), "spawned the Reading handler task");
            tx_reading.send(()).unwrap(); // safe; the channel was just opened

            // these objects are sent from `Node::adapt_stream`
            while let Some((mut conn, conn_returner)) = conn_receiver.recv().await {
                let addr = conn.addr;
                let mut reader = conn.reader.take().unwrap(); // safe; it is available at this point
                let mut buffer = Vec::new();

                let (inbound_message_sender, mut inbound_message_receiver) =
                    mpsc::channel(self_clone.node().config().inbound_queue_depth);

                // Use a channel to know when the processing task is ready.
                let (tx_processing, rx_processing) = oneshot::channel::<()>();

                // the task for processing parsed messages
                let processing_clone = self_clone.clone();
                let inbound_processing_task = tokio::spawn(async move {
                    let node = processing_clone.node();
                    trace!(parent: node.span(), "spawned a task for processing messages from {}", addr);
                    tx_processing.send(()).unwrap(); // safe; the channel was just opened

                    while let Some(msg) = inbound_message_receiver.recv().await {
                        if let Err(e) = processing_clone.process_message(addr, msg).await {
                            error!(parent: node.span(), "can't process a message from {}: {}", addr, e);
                            node.known_peers().register_failure(addr);
                        }
                    }
                });
                let _ = rx_processing.await;
                conn.tasks.push(inbound_processing_task);

                // Use a channel to know when the reader task is ready.
                let (tx_reader, rx_reader) = oneshot::channel::<()>();

                // the task for reading messages from a stream
                let reader_clone = self_clone.clone();
                let reader_task = tokio::spawn(async move {
                    let node = reader_clone.node();
                    trace!(parent: node.span(), "spawned a task for reading messages from {}", addr);
                    tx_reader.send(()).unwrap(); // safe; the channel was just opened

                    // postpone reads until the connection is fully established; if the process fails,
                    // this task gets aborted, so there is no need for a dedicated timeout
                    while !node.connected_addrs().contains(&addr) {
                        sleep(Duration::from_millis(1)).await;
                    }

                    loop {
                        if let Err(e) = reader_clone
                            .read_from_stream(
                                addr,
                                &mut buffer,
                                &mut reader,
                                &inbound_message_sender,
                            )
                            .await
                        {
                            node.known_peers().register_failure(addr);
                            buffer.clear();
                            if node.config().fatal_io_errors.contains(&e.kind()) {
                                node.disconnect(addr).await;
                                break;
                            } else {
                                sleep(Duration::from_secs(node.config().invalid_read_delay_secs))
                                    .await;
                            }
                        }
                    }
                });
                let _ = rx_reader.await;
                conn.tasks.push(reader_task);

                // return the Connection to the Node, resuming Node::adapt_stream
                if conn_returner.send(Ok(conn)).is_err() {
                    unreachable!("could't return a Connection to the Node");
                }
            }
        });
        let _ = rx_reading.await;
        self.node().tasks.lock().push(reading_task);

        // register the ReadingHandler with the Node
        let hdl = ReadingHandler(conn_sender);
        assert!(
            self.node().protocols.reading_handler.set(hdl).is_ok(),
            "the Reading protocol was enabled more than once!"
        );
    }

    /// Performs a read from the given reader. The default implementation is buffered; it sacrifices a bit of
    /// simplicity for better performance.
    async fn read_from_stream<R: AsyncRead + Unpin + Send>(
        &self,
        addr: SocketAddr,
        buffer: &mut Vec<u8>,
        reader: &mut R,
        message_sender: &mpsc::Sender<Self::Message>,
    ) -> io::Result<()> {
        // register the number of bytes carried over from the previous read (if there were any)
        let carry = buffer.len();

        // limit the maximum number of bytes that can be read
        let max_read_size = self.node().config().read_buffer_size - carry;
        let mut read_handle = reader.take(max_read_size as u64);

        // perform a read from the stream into the provided buffer
        let read_result = read_handle.read_buf(buffer).await;

        match read_result {
            Ok(0) => Err(io::ErrorKind::UnexpectedEof.into()),
            Err(e) => {
                error!(parent: self.node().span(), "can't read from {}: {}", addr, e);
                buffer.clear();
                Err(e)
            }
            Ok(read_len) => {
                // the number of bytes left to *process* - this includes the initial carried bytes and the read
                let left = carry + read_len;

                trace!(parent: self.node().span(), "read {}B from {}; {}B waiting to be processed", read_len, addr, left);

                self.process_buffer(addr, buffer, left, message_sender)
            }
        }
    }

    /// Attempts to isolate full messages from the connection's read buffer using `Reading::read_message`. Once
    /// no more messages can be extracted, it preserves any leftover bytes and moves them to the beginning of the
    /// buffer, and further reads from the stream are appended to them. Read messages are sent to a separate message
    /// processing task in order not to block further reads.
    fn process_buffer(
        &self,
        addr: SocketAddr,
        buffer: &mut Vec<u8>,
        mut left: usize,
        message_sender: &mpsc::Sender<Self::Message>,
    ) -> io::Result<()> {
        // wrap the read buffer in a reader
        let mut buf_reader = io::Cursor::new(&buffer[..left]);

        // several messages could have been read at once; process the contents of the buffer
        loop {
            // the position in the buffer before the message read attempt
            let initial_buf_pos = buf_reader.position() as usize;

            // try to read a single message from the buffer
            let read = self.read_message(addr, &mut buf_reader);

            // the position in the buffer after the read attempt
            let post_read_buf_pos = buf_reader.position() as usize;

            // register the number of bytes that were processed by the Reading::read_message call above
            let parse_size = post_read_buf_pos - initial_buf_pos;

            match read {
                // a full message was read successfully
                Ok(Some(msg)) => {
                    // subtract the number of successfully processed bytes from the ones left to process
                    left -= parse_size;

                    trace!(parent: self.node().span(), "isolated {}B as a message from {}", parse_size, addr);

                    self.node()
                        .known_peers()
                        .register_received_message(addr, parse_size);
                    self.node().stats().register_received_message(parse_size);

                    // send the message for further processing
                    if let Err(e) = message_sender.try_send(msg) {
                        error!(parent: self.node().span(), "can't process a message from {}: {}", addr, e);
                        self.node().stats().register_failure();
                    }

                    // if the read is exhausted, clear the read buffer and return
                    if left == 0 {
                        buffer.clear();
                        return Ok(());
                    }
                }
                // the message in the buffer is incomplete
                Ok(None) => {
                    // forbid messages that are larger than the read buffer
                    if left > self.node().config().read_buffer_size {
                        error!(parent: self.node().span(), "a message from {} is too large", addr);
                        buffer.clear();
                        return Err(io::ErrorKind::InvalidData.into());
                    }

                    trace!(parent: self.node().span(), "incomplete message from {}; carrying {}B over", addr, left);

                    // move the leftover bytes to the beginning of the buffer; the next read will append bytes
                    // starting from where the leftover ones end, allowing the message to be completed
                    buffer.copy_within(initial_buf_pos..initial_buf_pos + left, 0);
                    buffer.truncate(left);

                    return Ok(());
                }
                // an erroneous message (e.g. an unexpected zero-length payload)
                Err(e) => {
                    error!(parent: self.node().span(), "a message from {} is invalid", addr);
                    buffer.clear();
                    return Err(e);
                }
            }
        }
    }

    /// Reads a single message from the given reader; `Ok(None)` indicates that the message is incomplete,
    /// i.e. further reads from the stream must be performed in order to produce the whole message. An `Err`
    /// returned here indicates an invalid message which, depending on the configured list of fatal errors,
    /// can cause the related connection to be dropped.
    fn read_message<R: io::Read>(
        &self,
        source: SocketAddr,
        reader: &mut R,
    ) -> io::Result<Option<Self::Message>>;

    /// Processes an inbound message. Can be used to update state, send replies etc.
    async fn process_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()>;
}

/// The handler object dedicated to the `Reading` protocol.
pub struct ReadingHandler(mpsc::Sender<ReturnableConnection>);

impl ReadingHandler {
    pub(crate) async fn trigger(&self, item: ReturnableConnection) {
        if self.0.send(item).await.is_err() {
            unreachable!(); // protocol's task is down! can't recover
        }
    }
}
