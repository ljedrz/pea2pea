use crate::{protocols::ReturnableConnection, Pea2Pea};

#[cfg(doc)]
use crate::{protocols::Handshake, Config};

use async_trait::async_trait;
use bytes::Buf;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::{mpsc, oneshot},
    time::sleep,
};
use tracing::*;

use std::{io, net::SocketAddr, time::Duration};

/// Can be used to specify and enable reading, i.e. receiving inbound messages. If the [`Handshake`]
/// protocol is enabled too, it goes into force only after the handshake has been concluded.
#[async_trait]
pub trait Reading: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// The final (deserialized) type of inbound messages.
    type Message: Send;

    /// Prepares the node to receive messages; failures to read from a connection's stream are penalized by a timeout
    /// defined in [`Config`], while the configured fatal errors result in an immediate disconnect (in order to e.g. avoid
    /// accidentally reading "borked" messages).
    async fn enable_reading(&self) {
        let (conn_sender, mut conn_receiver) = mpsc::unbounded_channel::<ReturnableConnection>();

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

                    'reading_loop: loop {
                        if let Err(e) = reader_clone
                            .read_from_stream(addr, &mut buffer, &mut reader)
                            .await
                        {
                            node.known_peers().register_failure(addr);
                            if node.config().fatal_io_errors.contains(&e.kind()) {
                                node.disconnect(addr).await;
                                break;
                            } else {
                                sleep(Duration::from_secs(node.config().invalid_read_delay_secs))
                                    .await;
                            }
                        } else {
                            while !buffer.is_empty() {
                                match reader_clone.process_buffer(addr, &mut buffer) {
                                    Ok(Some(msg)) => {
                                        // send the message for further processing
                                        if let Err(e) = inbound_message_sender.try_send(msg) {
                                            error!(parent: node.span(), "can't process a message from {}: {}", addr, e);
                                            node.stats().register_failure();
                                        }
                                    }
                                    Ok(None) => break,
                                    Err(e) => {
                                        node.known_peers().register_failure(addr);
                                        if node.config().fatal_io_errors.contains(&e.kind()) {
                                            node.disconnect(addr).await;
                                            break 'reading_loop;
                                        }
                                        buffer.clear();
                                    }
                                }
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
    ) -> io::Result<()> {
        // register the number of bytes carried over from the previous read (if there were any)
        let carry = buffer.len();

        // limit the maximum number of bytes that can be read
        let max_read_size = self.node().config().read_buffer_size - carry;

        // forbid messages that are larger than the read buffer
        if max_read_size == 0 {
            error!(parent: self.node().span(), "a message from {} is too large", addr);
            return Err(io::ErrorKind::InvalidData.into());
        }

        // perform a bounded read from the stream into the provided buffer
        let read_result = reader.take(max_read_size as u64).read_buf(buffer).await;

        match read_result {
            Ok(0) => Err(io::ErrorKind::UnexpectedEof.into()),
            Err(e) => {
                error!(parent: self.node().span(), "can't read from {}: {}", addr, e);
                Err(e)
            }
            Ok(read_len) => {
                let left = carry + read_len;
                trace!(parent: self.node().span(), "read {}B from {}; {}B waiting to be processed", read_len, addr, left);
                Ok(())
            }
        }
    }

    /// Attempts to isolate a full message from the connection's read buffer using [`Reading::read_message`].
    /// It preserves any leftover bytes and moves them to the beginning of the buffer, and further reads from
    /// the stream are appended to them.
    fn process_buffer(
        &self,
        addr: SocketAddr,
        buffer: &mut Vec<u8>,
    ) -> io::Result<Option<Self::Message>> {
        // wrap the read buffer in a reader
        let mut buf_reader = io::Cursor::new(&buffer);

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
                trace!(parent: self.node().span(), "isolated {}B as a message from {}", parse_size, addr);

                self.node()
                    .known_peers()
                    .register_received_message(addr, parse_size);
                self.node().stats().register_received_message(parse_size);

                // if the read is exhausted, clear the read buffer and return
                if parse_size == buffer.len() {
                    buffer.clear();
                } else {
                    buffer.copy_within(post_read_buf_pos.., 0);
                    buffer.truncate(buffer.len() - parse_size);
                }

                Ok(Some(msg))
            }
            // the message in the buffer is incomplete
            Ok(None) => {
                trace!(parent: self.node().span(), "incomplete message from {}; carrying {}B over", addr, buffer.len());
                Ok(None)
            }
            // an erroneous message (e.g. an unexpected zero-length payload)
            Err(e) => {
                error!(parent: self.node().span(), "a message from {} is invalid", addr);
                Err(e)
            }
        }
    }

    /// Reads a single message from the given in-memory reader; `Ok(None)` indicates that the message ,
    /// is incomplete i.e. further reads from the stream must be performed in order to produce the whole
    /// message. An `Err`returned here indicates an invalid message which, depending on the configured
    /// list of fatal errors, can cause the related connection to be dropped.
    ///
    /// note: The maximum size of inbound messages is automatically enforced via [`Config::read_buffer_size`],
    /// but your implementation is free to impose a limit lower than the size of the buffer.
    fn read_message<R: Buf>(
        &self,
        source: SocketAddr,
        reader: &mut R,
    ) -> io::Result<Option<Self::Message>>;

    /// Processes an inbound message. Can be used to update state, send replies etc.
    async fn process_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()>;
}

/// The handler object dedicated to the [`Reading`] protocol.
pub struct ReadingHandler(mpsc::UnboundedSender<ReturnableConnection>);

impl ReadingHandler {
    pub(crate) fn trigger(&self, item: ReturnableConnection) {
        if self.0.send(item).is_err() {
            unreachable!(); // protocol's task is down! can't recover
        }
    }
}
