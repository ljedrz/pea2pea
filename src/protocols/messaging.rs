use crate::{ConnectionReader, ContainsNode};

use async_trait::async_trait;
use tokio::{sync::mpsc::channel, task::JoinHandle, time::sleep};
use tracing::*;

use std::{io, net::SocketAddr, time::Duration};

#[async_trait]
pub trait MessagingProtocol: ContainsNode
where
    Self: Clone + Send + 'static,
{
    type Message;

    fn enable_messaging_protocol(&self) {
        let (sender, mut receiver) = channel(self.node().config.inbound_message_queue_depth);
        self.node().set_inbound_messages(sender);

        let self_clone = self.clone();
        tokio::spawn(async move {
            let node = self_clone.node();
            loop {
                if let Some((request, source)) = receiver.recv().await {
                    if let Some(msg) = self_clone.parse_message(&request) {
                        self_clone.process_message(&msg);

                        if let Err(e) = self_clone.respond_to_message(msg, source) {
                            error!(parent: node.span(), "failed to handle an inbound message: {}", e);
                        }
                    } else {
                        error!(parent: node.span(), "can't parse an inbound message");
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
                            debug!(parent: node.span(), "received a message from {}", addr);

                            node.register_received_message(addr, msg.len());

                            if let Some(ref inbound_messages) = node.inbound_messages() {
                                if let Err(e) = inbound_messages.send((msg, addr)).await {
                                    error!(parent: node.span(), "can't process an inbound message: {}", e);
                                    // TODO: how to proceed?
                                }
                            }
                        }
                        Err(e) => {
                            node.register_failure(addr);
                            error!(parent: node.span(), "can't read message: {}", e);
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

    fn parse_message(&self, buffer: &[u8]) -> Option<Self::Message>;

    fn process_message(&self, _message: &Self::Message) {
        // do nothing by default
    }

    fn respond_to_message(&self, _message: Self::Message, _source: SocketAddr) -> io::Result<()> {
        // don't do anything by default
        Ok(())
    }
}

pub type MessagingClosure =
    Box<dyn Fn(ConnectionReader, SocketAddr) -> JoinHandle<()> + Send + Sync>;
