use crate::{ConnectionReader, ContainsNode};

use async_trait::async_trait;
use tokio::{sync::mpsc::channel, task::JoinHandle, time::sleep};
use tracing::*;

use std::{any::Any, io, net::SocketAddr, time::Duration};

#[async_trait]
pub trait MessagingProtocol: ContainsNode
where
    Self: Clone + Send + 'static,
    Self::Message: Send,
{
    type Message;

    fn enable_messaging_protocol(&self) {
        let (sender, mut receiver) = channel(self.node().config.inbound_message_queue_depth);
        self.node().set_inbound_messages(sender);

        let self_clone = self.clone();
        tokio::spawn(async move {
            let node = self_clone.node();
            loop {
                if let Some((source, msg)) = receiver.recv().await {
                    let msg = *msg.downcast().unwrap();
                    self_clone.process_message(source, &msg);

                    if let Err(e) = self_clone.respond_to_message(source, msg) {
                        error!(parent: node.span(), "failed to respond to an inbound message: {}", e);
                        node.register_failure(source);
                    }
                }
            }
        });

        let reading_closure = |mut connection_reader: ConnectionReader,
                               addr: SocketAddr|
         -> JoinHandle<()> {
            tokio::spawn(async move {
                let node = connection_reader.node.clone();
                debug!(parent: node.span(), "spawned a task reading messages from {}", addr);
                loop {
                    match Self::read_message(&mut connection_reader).await {
                        Ok(msg) => {
                            node.register_received_message(addr, msg.len());

                            if let Some(ref inbound_messages) = node.inbound_messages() {
                                if let Some(msg) = Self::parse_message(addr, &msg) {
                                    let boxed_msg = Box::new(msg) as Box<dyn Any + Send>;
                                    if let Err(e) = inbound_messages.send((addr, boxed_msg)).await {
                                        error!(parent: node.span(), "can't process an inbound message: {}", e);
                                        // TODO: how to proceed?
                                    }
                                } else {
                                    error!(parent: node.span(), "can't parse an inbound message");
                                    node.register_failure(addr);
                                }
                            }
                        }
                        Err(e) => {
                            node.register_failure(addr);
                            error!(parent: node.span(), "can't read a message from {}: {}", addr, e);
                            sleep(Duration::from_secs(
                                node.config.invalid_message_penalty_secs,
                            ))
                            .await;
                        }
                    }
                }
            })
        };

        self.node().set_messaging_closure(Box::new(reading_closure));
    }

    async fn read_message(conn_reader: &mut ConnectionReader) -> io::Result<Vec<u8>>;

    fn parse_message(source: SocketAddr, buffer: &[u8]) -> Option<Self::Message>;

    #[allow(unused_variables)]
    fn process_message(&self, source: SocketAddr, message: &Self::Message) {
        // do nothing by default
    }

    #[allow(unused_variables)]
    fn respond_to_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()> {
        // don't do anything by default
        Ok(())
    }
}

pub type InboundMessage = Box<dyn Any + Send>;

pub type MessagingClosure =
    Box<dyn Fn(ConnectionReader, SocketAddr) -> JoinHandle<()> + Send + Sync>;
