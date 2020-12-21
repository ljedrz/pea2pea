use crate::{ConnectionReader, Node};

use async_trait::async_trait;
use tokio::task::JoinHandle;
use tracing::*;

use std::{io, net::SocketAddr};

#[async_trait]
pub trait ReadProtocol {
    fn enable_reading_protocol(&self)
    where
        Self: Send + Sync,
    {
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

                            // node.known_peers.register_incoming_message(addr, msg.len());

                            if let Some(ref incoming_requests) = node.incoming_requests() {
                                if let Err(e) = incoming_requests.send((msg, addr)).await {
                                    error!(parent: node.span(), "can't register an incoming message: {}", e);
                                    // TODO: how to proceed?
                                }
                            }
                        }
                        Err(e) => {
                            // node.known_peers.register_failure(addr);
                            error!(parent: node.span(), "can't read message: {}", e);
                        }
                    }
                }
            })
        };

        self.node().set_reading_closure(Box::new(reading_closure));
    }

    fn node(&self) -> &Node;

    async fn read_message(conn_reader: &mut ConnectionReader) -> io::Result<Vec<u8>>;
}

pub type ReadingClosure =
    Box<dyn Fn(ConnectionReader, SocketAddr) -> JoinHandle<()> + Send + Sync + 'static>;
