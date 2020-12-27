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

#[async_trait]
pub trait Messaging: ContainsNode
where
    Self: Clone + Send + Sync + 'static,
    Self::Message: Send,
{
    type Message;

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
                        if let Some(msg) = self_clone.parse_message(source, &msg) {
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

        let reading_closure =
            |mut connection_reader: ConnectionReader, addr: SocketAddr| -> JoinHandle<()> {
                tokio::spawn(async move {
                    let node = connection_reader.node.clone();
                    debug!(parent: node.span(), "spawned a task reading messages from {}", addr);

                    // the number of bytes carried over from an incomplete message
                    let mut carry = 0;

                    loop {
                        match connection_reader
                            .reader
                            .read(&mut connection_reader.buffer[carry..])
                            .await
                        {
                            Ok(n) => {
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

    fn read_message(buffer: &[u8]) -> Option<&[u8]>;

    fn parse_message(&self, source: SocketAddr, buffer: &[u8]) -> Option<Self::Message>;

    #[allow(unused_variables)]
    fn process_message(&self, source: SocketAddr, message: &Self::Message) {
        // do nothing by default
    }

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

pub type InboundMessage = Vec<u8>;
pub type InboundMessages = Sender<(SocketAddr, InboundMessage)>;
pub type ReadingClosure = Box<dyn Fn(ConnectionReader, SocketAddr) -> JoinHandle<()> + Send + Sync>;
