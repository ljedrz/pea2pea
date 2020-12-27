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
    Self::Message: Send,
{
    /// The (final) type of the inbound messages.
    type Message;

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
                        let node = self_clone.node();
                        if let Some(msg) = self_clone.parse_message(source, msg) {
                            self_clone.process_message(source, &msg);

                            if let Err(e) = self_clone.respond_to_message(source, msg).await {
                                error!(parent: node.span(), "failed to respond to an inbound message: {}", e);
                                node.register_failure(source);
                            }
                        } else {
                            error!(parent: node.span(), "can't parse an inbound message");
                            node.register_failure(source);
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

                            while let Some(msg) = Self::read_message(
                                &connection_reader.buffer[processed..processed + left],
                            ) {
                                node.register_received_message(addr, msg.len());

                                if let Some(ref inbound_messages) = node.inbound_messages() {
                                    // can't recover from an error here
                                    inbound_messages
                                        .send((addr, msg.to_vec()))
                                        .await
                                        .expect("the inbound message channel is closed")
                                }

                                processed += msg.len();
                                left -= msg.len();
                            }
                            // if no bytes were processed, there must have been invalid messages;
                            // register an error, set carry to zero and move to the next read
                            if processed == 0 {
                                node.register_failure(addr);
                                carry = 0;
                                continue;
                            }
                            connection_reader
                                .buffer
                                .copy_within(processed..processed + left, 0);
                            carry = left;
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
    fn read_message(buffer: &[u8]) -> Option<&[u8]>;

    /// Deserializes a message from bytes.
    fn parse_message(&self, source: SocketAddr, message: Vec<u8>) -> Option<Self::Message>;

    /// Processes an inbound message.
    #[allow(unused_variables)]
    fn process_message(&self, source: SocketAddr, message: &Self::Message) {
        // do nothing by default
    }

    /// Responds to an inbound message.
    #[allow(unused_variables)]
    async fn respond_to_message(
        &self,
        source: SocketAddr,
        message: Self::Message,
    ) -> io::Result<()> {
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
