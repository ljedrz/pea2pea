use crate::{
    connections::{Connection, ConnectionReader},
    Pea2Pea,
};

use async_trait::async_trait;
use tokio::{
    io::AsyncReadExt,
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::sleep,
};
use tracing::*;

use std::{io, net::SocketAddr, time::Duration};

/// This protocol can be used to specify and enable reading, i.e. receiving inbound messages.
/// If handshaking is enabled too, it goes into force only after the handshake has been concluded.
#[async_trait]
pub trait Reading: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// The final type of incoming messages.
    type Message: Send;

    // TODO: add an associated type defaulting to ConnectionReader once
    // https://github.com/rust-lang/rust/issues/29661 is resolved.

    /// Prepares the node to receive messages.
    fn enable_reading(&self) {
        let (conn_sender, mut conn_receiver) =
            mpsc::channel::<ReadingObjects>(self.node().config.reading_handler_queue_depth);

        // the task spawning tasks reading messages from the given stream
        let self_clone = self.clone();
        let reading_task = tokio::spawn(async move {
            loop {
                if let Some((mut conn_reader, conn, conn_returner)) = conn_receiver.recv().await {
                    let addr = conn.addr;

                    let (inbound_message_sender, mut inbound_message_receiver) =
                        mpsc::channel(self_clone.node().config.conn_inbound_queue_depth);

                    // the task for reading messages from the stream
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
                                    error!(parent: node.span(), "failed to respond to an inbound message: {}", e);
                                    node.known_peers().register_failure(addr);
                                }
                            }
                        }
                    });

                    conn.reader_task
                        .set(reader_task)
                        .expect("reader_task was set twice!");
                    conn.inbound_processing_task
                        .set(inbound_processing_task)
                        .expect("inbound_processing_task was set twice!");

                    if conn_returner.send(Ok(conn)).is_err() {
                        // can't recover if this happens
                        panic!("can't return a Connection to the Node");
                    }
                }
            }
        });

        self.node()
            .set_reading_handler((conn_sender, reading_task).into());
    }

    /// Performs a read from the stream. The default implementation is buffered; it sacrifices a bit of simplicity for
    /// better performance. A naive approach would be to read only the number of bytes expected for a single message
    /// (if all of them have a fixed size) or first the number of bytes expected for a header, and then the number of
    /// bytes of the payload, as specified by the header.
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

        match reader.read(&mut buffer[*carry..]).await {
            Ok(n) => {
                trace!(parent: node.span(), "read {}B from {}", n, addr);
                let mut processed = 0;
                let mut left = *carry + n;

                // several messages could have been read at once; process the contents of the bufer
                loop {
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
                        // an incomplete message
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
            Err(e) => {
                error!(parent: node.span(), "can't read from {}: {}", addr, e);
                Err(io::ErrorKind::Other.into())
            }
        }
    }

    /// Reads a single inbound message from the given buffer; `Ok(None)` indicates that the message is incomplete,
    /// i.e. another read from the stream must be performed in order to produce the whole message. Alongside the
    /// message it returns the number of bytes it occupied in the buffer.
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

/// A set of objects required to enable the `Reading` protocol.
pub type ReadingObjects = (
    ConnectionReader,
    Connection,
    oneshot::Sender<io::Result<Connection>>,
);

/// An object dedicated to spawning inbound message handlers; used in the `Reading` protocol.
pub struct ReadingHandler {
    sender: mpsc::Sender<ReadingObjects>,
    _task: JoinHandle<()>,
}

impl ReadingHandler {
    /// Sends reading-relevant objects to the reading handler.
    pub async fn send(&self, reading_objects: ReadingObjects) {
        if self.sender.send(reading_objects).await.is_err() {
            // can't recover if this happens
            panic!("the inbound message handling task is down or its Receiver is closed")
        }
    }
}

impl From<(mpsc::Sender<ReadingObjects>, JoinHandle<()>)> for ReadingHandler {
    fn from((sender, _task): (mpsc::Sender<ReadingObjects>, JoinHandle<()>)) -> Self {
        Self { sender, _task }
    }
}
