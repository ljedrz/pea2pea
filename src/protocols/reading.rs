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
    /// The final (deserialized) type of incoming messages.
    type Message: Send;

    /// Prepares the node to receive messages; failures to read from a connection's stream are penalized by a timeout
    /// defined in `NodeConfig`, while broken/unreadable messages result in an immediate disconnect (in order to avoid
    /// accidentally reading "borked" messages).
    fn enable_reading(&self) {
        let (conn_sender, mut conn_receiver) = mpsc::channel::<ReturnableConnection>(
            self.node().config().protocol_handler_queue_depth,
        );

        // the main task spawning per-connection tasks reading messages from their streams
        let self_clone = self.clone();
        let reading_task = tokio::spawn(async move {
            trace!(parent: self_clone.node().span(), "spawned the Reading handler task");

            loop {
                // these objects are sent from `Node::adapt_stream`
                if let Some((mut conn, conn_returner)) = conn_receiver.recv().await {
                    let addr = conn.addr;
                    let mut reader = conn.reader.take().unwrap(); // safe; it is available at this point
                    let mut buffer = vec![0; self_clone.node().config().conn_read_buffer_size]
                        .into_boxed_slice();

                    let (inbound_message_sender, mut inbound_message_receiver) =
                        mpsc::channel(self_clone.node().config().conn_inbound_queue_depth);

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
                            } else {
                                node.disconnect(addr);
                                break;
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

                        let mut carry = 0;
                        loop {
                            match reader_clone
                                .read_from_stream(
                                    addr,
                                    &mut buffer,
                                    &mut reader,
                                    carry,
                                    &inbound_message_sender,
                                )
                                .await
                            {
                                Ok(leftover) => {
                                    carry = leftover;
                                }
                                Err(e) => {
                                    node.known_peers().register_failure(addr);
                                    match e.kind() {
                                        io::ErrorKind::InvalidData | io::ErrorKind::BrokenPipe => {
                                            // can't recover; drop the connection
                                            node.disconnect(addr);
                                            break;
                                        }
                                        io::ErrorKind::Other => {
                                            // an unsuccessful read from the stream is not fatal; instead of disconnecting,
                                            // impose a delay before attempting another read
                                            sleep(Duration::from_secs(
                                                node.config().invalid_read_delay_secs,
                                            ))
                                            .await;
                                        }
                                        _ => unreachable!(),
                                    }
                                }
                            }
                        }
                    });
                    conn.tasks.push(reader_task);

                    // return the Connection to the Node, resuming Node::adapt_stream
                    if conn_returner.send(Ok(conn)).is_err() {
                        unreachable!("could't return a Connection to the Node");
                    }
                } else {
                    error!("the Reading protocol is down!");
                    break;
                }
            }
        });

        // register the ReadingHandler with the Node
        self.node()
            .set_reading_handler((conn_sender, reading_task).into());
    }

    /// Performs a read from the given reader. The default implementation is buffered; it sacrifices a bit of
    /// simplicity for better performance. Read messages are sent to a message processing task in order to enable
    /// faster reads. Returns the number of pending bytes left in the buffer in case of an incomplete read; they
    /// should be provided to the medthod on the next call as `carry`.
    async fn read_from_stream<R: AsyncRead + Unpin + Send>(
        &self,
        addr: SocketAddr,
        buffer: &mut [u8],
        reader: &mut R,
        carry: usize,
        message_sender: &mpsc::Sender<Self::Message>,
    ) -> io::Result<usize> {
        // perform a read from the stream, being careful not to overwrite any bytes carried over from the previous read
        match reader.read(&mut buffer[carry..]).await {
            Ok(0) => return Ok(carry),
            Ok(n) => {
                trace!(parent: self.node().span(), "read {}B from {}", n, addr);
                let mut processed = 0;
                let mut left = carry + n;

                // several messages could have been read at once; process the contents of the buffer
                loop {
                    // try to read a single message from the buffer
                    match self.read_message(addr, &buffer[processed..processed + left]) {
                        // a full message was read successfully
                        Ok(Some((msg, len))) => {
                            // advance the counters
                            processed += len;
                            left -= len;

                            trace!(
                                parent: self.node().span(),
                                "isolated {}B as a message from {}; {}B left to process",
                                len,
                                addr,
                                left
                            );
                            self.node()
                                .known_peers()
                                .register_received_message(addr, len);
                            self.node().stats().register_received_message(len);

                            // send the message for further processing
                            if message_sender.send(msg).await.is_err() {
                                error!(parent: self.node().span(), "the inbound message channel is closed");
                                return Err(io::ErrorKind::BrokenPipe.into());
                            }

                            // if the read is exhausted, reset the carry and return
                            if left == 0 {
                                return Ok(0);
                            }
                        }
                        // the message in the buffer is incomplete
                        Ok(None) => {
                            // forbid messages that are larger than the read buffer
                            if left >= buffer.len() {
                                error!(parent: self.node().span(), "a message from {} is too large", addr);
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
                            buffer.copy_within(processed..processed + left, 0);

                            return Ok(left);
                        }
                        // an erroneous message (e.g. an unexpected zero-length payload)
                        Err(_) => {
                            error!(parent: self.node().span(), "a message from {} is invalid", addr);
                            return Err(io::ErrorKind::InvalidData.into());
                        }
                    }
                }
            }
            // a stream read error
            Err(e) => {
                error!(parent: self.node().span(), "can't read from {}: {}", addr, e);
                Err(io::ErrorKind::Other.into())
            }
        }
    }

    /// Reads a single message from the given buffer; `Ok(None)` indicates that the message is
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
