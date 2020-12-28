use crate::{ConnectionReader, ContainsNode};

use async_trait::async_trait;
use tokio::{
    io::AsyncReadExt,
    sync::mpsc::{channel, Sender},
    task::JoinHandle,
    time::sleep,
};
use tracing::*;

use std::{io, net::SocketAddr, time::Duration};

/// This protocol can be used to specify and enable messaging, i.e. handling of inbound messages and replying to them.
/// If handshaking is enabled too, it goes into force only after the handshake has been concluded.
#[async_trait]
pub trait Messaging: ContainsNode
where
    Self: Clone + Send + Sync + 'static,
{
    /// Prepares the node to receive messages and optionally respond to them.
    fn enable_messaging(&self) {
        let (sender, mut receiver) = channel(self.node().config.inbound_message_queue_depth);
        self.node().set_inbound_messages(sender);

        let self_clone = self.clone();
        tokio::spawn(async move {
            loop {
                if let Some((source, msg)) = receiver.recv().await {
                    let self_clone = self_clone.clone();
                    tokio::spawn(async move {
                        if let Err(e) = self_clone.process_message(source, msg).await {
                            error!(parent: self_clone.node().span(), "failed to respond to an inbound message: {}", e);
                            self_clone.node().register_failure(source);
                        }
                    });
                }
            }
        });

        let reading_closure = |mut connection_reader: ConnectionReader,
                               addr: SocketAddr|
         -> JoinHandle<()> {
            tokio::spawn(async move {
                let node = connection_reader.node.clone();
                trace!(parent: node.span(), "spawned a task for reading messages from {}", addr);

                loop {
                    if let Err(e) = Self::read_from_stream(&mut connection_reader).await {
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
            })
        };

        self.node().set_reading_closure(Box::new(reading_closure));
    }

    /// Performs a read from the stream. The default implementation is buffered; it sacrifices a bit of simplicity for
    /// better performance. A naive approach would be to read only the number of bytes expected for a single message
    /// (if all of them have a fixed size) or first the number of bytes expected for a header, and then the number of
    /// bytes of the payload, as specified by the header.
    async fn read_from_stream(conn_reader: &mut ConnectionReader) -> io::Result<()> {
        let ConnectionReader {
            node,
            peer_addr,
            reader,
            buffer,
            carry,
        } = conn_reader;

        match reader.read(&mut buffer[*carry..]).await {
            Ok(n) => {
                trace!(parent: node.span(), "read {}B from {}", n, peer_addr);
                let mut processed = 0;
                let mut left = *carry + n;

                // several messages could have been read at once; process the contents of the bufer
                loop {
                    match Self::read_message(&buffer[processed..processed + left]) {
                        // a full message was read successfully
                        Ok(Some(msg)) => {
                            node.register_received_message(*peer_addr, msg.len());

                            // send the message for further processing
                            if let Some(ref inbound_messages) = node.inbound_messages() {
                                // can't recover from an error here
                                inbound_messages
                                    .send((*peer_addr, msg.to_vec()))
                                    .await
                                    .expect("the inbound message channel is closed")
                            }

                            // advance the buffer
                            processed += msg.len();
                            left -= msg.len();

                            if left == 0 {
                                *carry = 0;
                            }
                        }
                        // a partial read; it gets carried over
                        Ok(None) => {
                            *carry = left;

                            // guard against messages that are larger than the read buffer
                            if *carry >= buffer.len() {
                                node.register_failure(*peer_addr);
                                error!(parent: node.span(), "a message from {} is too large", peer_addr);

                                return Err(io::ErrorKind::InvalidData.into());
                            }

                            // rotate the buffer so the next read can complete the message
                            buffer.copy_within(processed..processed + left, 0);

                            return Ok(());
                        }
                        // an erroneous message (e.g. an unexpected zero-length payload)
                        Err(_) => {
                            node.register_failure(*peer_addr);
                            error!(parent: node.span(), "a message from {} is invalid", peer_addr);

                            return Err(io::ErrorKind::InvalidData.into());
                        }
                    }
                }
            }
            Err(e) => {
                node.register_failure(*peer_addr);
                error!(parent: node.span(), "can't read from {}: {}", peer_addr, e);

                Err(io::ErrorKind::Other.into())
            }
        }
    }

    /// Reads a single inbound message from the given buffer; `Ok(None)` indicates that the message is incomplete,
    /// i.e. another read from the stream must be performed in order to produce the whole message.
    fn read_message(buffer: &[u8]) -> io::Result<Option<&[u8]>>;

    /// Processes an inbound message. Can be used to update state, send replies etc.
    #[allow(unused_variables)]
    async fn process_message(&self, source: SocketAddr, message: Vec<u8>) -> io::Result<()> {
        // don't do anything by default
        Ok(())
    }
}

/// The type transmitted using the `InboundMessages` sender.
pub type InboundMessage = Vec<u8>;

/// A sender used to transmit inbound messages from the reader task for further handling by the node.
pub type InboundMessages = Sender<(SocketAddr, InboundMessage)>;

/// The closure used to receive inbound messages from every connection.
pub type ReadingClosure = Box<dyn Fn(ConnectionReader, SocketAddr) -> JoinHandle<()> + Send + Sync>;
