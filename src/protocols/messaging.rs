use crate::{ConnectionReader, Pea2Pea};

use async_trait::async_trait;
use tokio::{io::AsyncReadExt, sync::mpsc, time::sleep};
use tracing::*;

use std::{io, net::SocketAddr, sync::Arc, time::Duration};

/// This protocol can be used to specify and enable messaging, i.e. handling of inbound messages and replying to them.
/// If handshaking is enabled too, it goes into force only after the handshake has been concluded.
#[async_trait]
pub trait Messaging: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// The final type of incoming messages.
    type Message: Send;

    /// Prepares the node to receive messages and optionally respond to them.
    fn enable_messaging(&self) {
        let (conn_reader_sender, mut conn_reader_receiver) =
            mpsc::channel(self.node().config.inbound_message_queue_depth);
        self.node().set_inbound_handler(conn_reader_sender.into());

        // the task spawning tasks reading messages from the given stream
        let self_clone = self.clone();
        tokio::spawn(async move {
            loop {
                if let Some(mut conn_reader) = conn_reader_receiver.recv().await {
                    let (inbound_message_sender, mut inbound_message_receiver) =
                        mpsc::channel(self_clone.node().config.inbound_message_queue_depth);
                    let addr = conn_reader.addr;

                    // the task for reading messages from the stream
                    let self_clone = self_clone.clone();
                    tokio::spawn(async move {
                        let node = Arc::clone(&conn_reader.node);
                        trace!(parent: node.span(), "spawned a task for reading messages from {}", addr);

                        loop {
                            if let Err(e) =
                                Self::read_from_stream(&mut conn_reader, &inbound_message_sender)
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
                    let self_clone = self_clone.clone();
                    tokio::spawn(async move {
                        loop {
                            if let Some(msg) = inbound_message_receiver.recv().await {
                                if let Err(e) = self_clone.process_message(addr, msg).await {
                                    error!(parent: self_clone.node().span(), "failed to respond to an inbound message: {}", e);
                                    self_clone.node().known_peers().register_failure(addr);
                                }
                            }
                        }
                    });
                }
            }
        });
    }

    /// Performs a read from the stream. The default implementation is buffered; it sacrifices a bit of simplicity for
    /// better performance. A naive approach would be to read only the number of bytes expected for a single message
    /// (if all of them have a fixed size) or first the number of bytes expected for a header, and then the number of
    /// bytes of the payload, as specified by the header.
    async fn read_from_stream(
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
                    match Self::read_message(&buffer[processed..processed + left]) {
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
    fn read_message(buffer: &[u8]) -> io::Result<Option<(Self::Message, usize)>>;

    /// Processes an inbound message. Can be used to update state, send replies etc.
    #[allow(unused_variables)]
    async fn process_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()> {
        // don't do anything by default
        Ok(())
    }
}

/// An object dedicated to handling inbound messages.
pub struct InboundHandler(mpsc::Sender<ConnectionReader>);

impl InboundHandler {
    /// Sends the connection reader to the task handling inbound messages.
    pub async fn send(&self, connection_reader: ConnectionReader) {
        if let Err(_) = self.0.send(connection_reader).await {
            // can't recover if this happens
            panic!("the inbound message handling task is down or its Receiver is closed")
        }
    }
}

impl From<mpsc::Sender<ConnectionReader>> for InboundHandler {
    fn from(sender: mpsc::Sender<ConnectionReader>) -> Self {
        Self(sender)
    }
}
