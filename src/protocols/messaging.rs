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

        // the default implementation is buffered; it sacrifices a bit of simplicity for better performance
        let reading_closure = |mut connection_reader: ConnectionReader,
                               addr: SocketAddr|
         -> JoinHandle<()> {
            tokio::spawn(async move {
                let node = connection_reader.node.clone();
                trace!(parent: node.span(), "spawned a task for reading messages from {}", addr);

                // the number of bytes carried over from an incomplete message
                let mut carry = 0;

                loop {
                    match connection_reader
                        .reader
                        .read(&mut connection_reader.buffer[carry..])
                        .await
                    {
                        Ok(n) => {
                            trace!(parent: node.span(), "read {}B from {}", n, addr);
                            let mut processed = 0;
                            let mut left = carry + n;

                            // several messages could have been read at once; process the contents of the bufer
                            loop {
                                match Self::read_message(
                                    &connection_reader.buffer[processed..processed + left],
                                ) {
                                    // a full message was read successfully
                                    Ok(Some(msg)) => {
                                        node.register_received_message(addr, msg.len());

                                        // send the message for further processing
                                        if let Some(ref inbound_messages) = node.inbound_messages()
                                        {
                                            // can't recover from an error here
                                            inbound_messages
                                                .send((addr, msg.to_vec()))
                                                .await
                                                .expect("the inbound message channel is closed")
                                        }

                                        // advance the buffer
                                        processed += msg.len();
                                        left -= msg.len();
                                    }
                                    // a partial read; it gets carried over
                                    Ok(None) => {
                                        carry = left;

                                        // guard against messages that are larger than the read buffer
                                        if carry >= connection_reader.buffer.len() {
                                            node.register_failure(addr);
                                            error!(parent: node.span(), "can't read from {}: the message is too large", addr);
                                            // drop the connection to avoid reading borked messages
                                            node.disconnect(addr);
                                            return;
                                        }

                                        // rotate the buffer so the next read can complete the message
                                        connection_reader
                                            .buffer
                                            .copy_within(processed..processed + left, 0);
                                        break;
                                    }
                                    // an erroneous message (e.g. an unexpected zero-length payload)
                                    Err(_) => {
                                        node.register_failure(addr);
                                        error!(parent: node.span(), "can't read from {}: invalid message", addr);
                                        // drop the connection to avoid reading borked messages
                                        node.disconnect(addr);
                                        return;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            node.register_failure(addr);
                            error!(parent: node.span(), "can't read from {}: {}", addr, e);
                            sleep(Duration::from_secs(
                                node.config.invalid_message_penalty_secs,
                            ))
                            .await;
                        }
                    }
                }
            })
        };

        self.node().set_reading_closure(Box::new(reading_closure));
    }

    /// Reads a single inbound message from the given buffer.
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
