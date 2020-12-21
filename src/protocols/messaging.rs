use crate::{ConnectionReader, ContainsNode};

use async_trait::async_trait;
use tokio::{sync::mpsc::channel, task::JoinHandle};
use tracing::*;

use std::{io, net::SocketAddr, sync::Arc};

#[async_trait]
pub trait MessagingProtocol: ContainsNode {
    type Message;

    fn enable_messaging_protocol(self: &Arc<Self>)
    where
        Self: Send + Sync + 'static,
    {
        let (sender, mut receiver) = channel(1024); // TODO: specify in NodeConfig
        self.node().set_inbound_messages(sender);

        let self_clone = Arc::clone(self);
        tokio::spawn(async move {
            let node = self_clone.node();
            loop {
                if let Some((request, source)) = receiver.recv().await {
                    if let Some(msg) = self_clone.parse_message(&request) {
                        self_clone.process_message(&msg);

                        if let Err(e) = self_clone.respond_to_message(msg, source) {
                            error!(parent: node.span(), "failed to handle an incoming message: {}", e);
                        }
                    } else {
                        error!(parent: node.span(), "can't parse an incoming message");
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
                            info!(parent: node.span(), "received a message from {}", addr);

                            node.register_received_message(addr, msg.len());

                            if let Some(ref inbound_messages) = node.inbound_messages() {
                                if let Err(e) = inbound_messages.send((msg, addr)).await {
                                    error!(parent: node.span(), "can't register an incoming message: {}", e);
                                    // TODO: how to proceed?
                                }
                            }
                        }
                        Err(e) => {
                            node.register_failure(addr);
                            error!(parent: node.span(), "can't read message: {}", e);
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

    fn respond_to_message(
        self: &Arc<Self>,
        _message: Self::Message,
        _source: SocketAddr,
    ) -> io::Result<()> {
        // don't do anything by default
        Ok(())
    }
}

pub type MessagingClosure =
    Box<dyn Fn(ConnectionReader, SocketAddr) -> JoinHandle<()> + Send + Sync + 'static>;
