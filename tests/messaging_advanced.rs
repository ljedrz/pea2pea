#![allow(dead_code)]

// WIP

use tokio::sync::mpsc::channel;
use tracing::*;

use pea2pea::{Node, ResponseProtocol};

use std::{io, net::SocketAddr, sync::Arc};

#[derive(Clone)]
struct AdvancedNode(Arc<Node>);

impl std::ops::Deref for AdvancedNode {
    type Target = Node;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

struct Version {
    pub version: u64,
    pub height: u32,
    pub nonce: u64,
    pub sender: SocketAddr,
    pub receiver: SocketAddr,
    pub timestamp: i64,
}

struct Verack {
    pub nonce: u64,
    pub sender: SocketAddr,
    pub receiver: SocketAddr,
}

pub struct TestMessage {
    name: [u8; 12],
    payload: TestMessagePayload,
}

enum TestMessagePayload {
    Version(Version),
    Verack(Verack),
}

impl ResponseProtocol for AdvancedNode {
    type Message = TestMessage;

    fn enable_response_protocol(self: &Arc<Self>) {
        let (sender, mut receiver) = channel(16);
        self.incoming_requests.set(Some(sender)).unwrap();

        let node = Arc::clone(&self);
        tokio::spawn(async move {
            loop {
                if let Some((request, source)) = receiver.recv().await {
                    if let Some(msg) = node.parse_message(&request) {
                        if node.validate_message(&msg) {
                            if let Err(e) = node.process_message(msg, source) {
                                // TODO: drop on fatal IO errors
                            }
                        } else {
                            error!("failed to validate an incoming message");
                            // TODO: drop associated connection
                        }
                    } else {
                        error!("can't parse an incoming message");
                        // TODO: drop associated connection
                    }
                }
            }
        });
    }

    fn parse_message(&self, buffer: &[u8]) -> Option<Self::Message> {
        let mut message_name = [0u8; 12];
        message_name.copy_from_slice(&buffer[0..12]);
        let buffer = &buffer[12..];

        let payload = match &message_name {
            b"version00000" => TestMessagePayload::Version(Version {
                version: bincode::deserialize(&buffer[..8]).ok()?,
                height: bincode::deserialize(&buffer[8..12]).ok()?,
                nonce: bincode::deserialize(&buffer[12..20]).ok()?,
                sender: bincode::deserialize(&buffer[20..30]).ok()?,
                receiver: bincode::deserialize(&buffer[30..40]).ok()?,
                timestamp: bincode::deserialize(&buffer[40..48]).ok()?,
            }),
            b"verack000000" => TestMessagePayload::Verack(Verack {
                nonce: bincode::deserialize(&buffer[..8]).ok()?,
                sender: bincode::deserialize(&buffer[8..18]).ok()?,
                receiver: bincode::deserialize(&buffer[18..28]).ok()?,
            }),
            _ => unreachable!(),
        };

        Some(Self::Message {
            name: message_name,
            payload,
        })
    }

    fn process_message(
        self: &Arc<Self>,
        message: TestMessage,
        source_addr: SocketAddr,
    ) -> io::Result<()> {
        match message.payload {
            TestMessagePayload::Version(version) => {
                let mut buffer = Vec::with_capacity(28);
                buffer.extend_from_slice(&bincode::serialize(&0u64).unwrap());
                buffer.extend_from_slice(&bincode::serialize(&self.local_addr).unwrap());
                buffer.extend_from_slice(&bincode::serialize(&source_addr).unwrap());

                let node = Arc::clone(self);
                tokio::spawn(async move {
                    node.send_direct_message(source_addr, false, buffer)
                        .await
                        .unwrap();
                });

                Ok(())
            }
            TestMessagePayload::Verack(verack) => Ok(()),
        }
    }
}

// TODO: implement
#[ignore]
#[tokio::test]
async fn request_handling() {
    let node = Node::new(None).await.unwrap();
}
