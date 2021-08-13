use crate::{protocols::ReturnableConnection, Pea2Pea};

use async_trait::async_trait;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::mpsc,
    time::sleep,
};
use tracing::*;

use std::{io, net::SocketAddr, time::Duration};

/// Can be used to specify and enable reading, i.e. receiving inbound messages.
/// If handshaking is enabled too, it goes into force only after the handshake has been concluded.
#[async_trait]
pub trait Reading: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// The final (deserialized) type of inbound messages.
    type Message: Send;

    /// Prepares the node to receive messages; failures to read from a connection's stream are penalized by a timeout
    /// defined in `Config`, while broken/unreadable messages result in an immediate disconnect (in order to avoid
    /// accidentally reading "borked" messages).
    fn enable_reading(&self) {
        let (conn_sender, mut conn_receiver) = mpsc::channel::<ReturnableConnection>(
            self.node().config().protocol_handler_queue_depth,
        );

        // the main task spawning per-connection tasks reading messages from their streams
        let self_clone = self.clone();
        let reading_task = tokio::spawn(async move {
            trace!(parent: self_clone.node().span(), "spawned the Reading handler task");

            // these objects are sent from `Node::adapt_stream`
            while let Some((mut conn, conn_returner)) = conn_receiver.recv().await {
                let addr = conn.addr;
                let mut reader = conn.reader.take().unwrap(); // safe; it is available at this point
                let mut buffer = Vec::new();

                let (inbound_message_sender, mut inbound_message_receiver) =
                    mpsc::channel(self_clone.node().config().inbound_queue_depth);

                // the task for processing parsed messages
                let processing_clone = self_clone.clone();
                let inbound_processing_task = tokio::spawn(async move {
                    let node = processing_clone.node();
                    trace!(parent: node.span(), "spawned a task for processing messages from {}", addr);

                    while let Some(msg) = inbound_message_receiver.recv().await {
                        if let Err(e) = processing_clone.process_message(addr, msg).await {
                            error!(parent: node.span(), "can't process a message from {}: {}", addr, e);
                            node.known_peers().register_failure(addr);
                        }
                    }
                });
                conn.tasks.push(inbound_processing_task);

                // the task for reading messages from a stream
                let reader_clone = self_clone.clone();
                let reader_task = tokio::spawn(async move {
                    let node = reader_clone.node();
                    trace!(parent: node.span(), "spawned a task for reading messages from {}", addr);

                    // postpone reads until the connection is fully established; if the process fails,
                    // this task gets aborted, so there is no need for a dedicated timeout
                    while !node.connected_addrs().contains(&addr) {
                        sleep(Duration::from_millis(5)).await;
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
                conn.tasks.push(reader_task);

                // return the Connection to the Node, resuming Node::adapt_stream
                if conn_returner.send(Ok(conn)).is_err() {
                    unreachable!("could't return a Connection to the Node");
                }
            }
        });
        self.node().tasks.lock().push(reading_task);

        // register the ReadingHandler with the Node
        self.node().set_reading_handler(conn_sender.into());
    }

    /// Performs a read from the given reader. The default implementation is buffered; it sacrifices a bit of
    /// simplicity for better performance. Read messages are sent to a message processing task in order to enable
    /// faster reads. Returns the number of pending bytes left in the buffer in case of an incomplete read; they
    /// should be provided to the medthod on the next call as `carry`.
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
        let mut handle = reader.take((self.node().config().read_buffer_size - carry) as u64);
        // perform a read from the stream
        let read = handle.read_buf(buffer).await;

        match read {
            Ok(0) => Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                trace!(parent: self.node().span(), "read {}B from {}", n, addr);
                let mut left = carry + n;

                // wrap the read buffer in a reader
                let mut buf_reader = io::Cursor::new(&mut buffer[..left]);

                // several messages could have been read at once; process the contents of the buffer
                loop {
                    // the position in the buffer before the message read attempt
                    let initial_buf_pos = buf_reader.position() as usize;

                    // try to read a single message from the buffer
                    match self.read_message(addr, &mut buf_reader) {
                        // a full message was read successfully
                        Ok(Some(msg)) => {
                            let len = buf_reader.position() as usize - initial_buf_pos;
                            left -= len;

                            trace!(
                                parent: self.node().span(),
                                "isolated {}B as a message from {}, {}B left",
                                len,
                                addr,
                                left,
                            );
                            self.node()
                                .known_peers()
                                .register_received_message(addr, len);
                            self.node().stats().register_received_message(len);

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

                            trace!(
                                parent: self.node().span(),
                                "a message from {} is incomplete; carrying {}B over",
                                addr,
                                left
                            );

                            // move the leftover bytes to the beginning of the buffer; the next read will append bytes
                            // starting from where the leftover ones end, allowing the message to be completed
                            let post_read_buf_pos = buf_reader.position() as usize;
                            buffer.copy_within(initial_buf_pos..post_read_buf_pos, 0);
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
            // a stream read error
            Err(e) => {
                error!(parent: self.node().span(), "can't read from {}: {}", addr, e);
                buffer.clear();
                Err(e)
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
    #[allow(unused_variables)]
    async fn process_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()> {
        // don't do anything by default
        Ok(())
    }
}
