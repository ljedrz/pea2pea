use tracing::*;

use pea2pea::{ContainsNode, Messaging, Node, NodeConfig};

use std::{convert::TryInto, net::SocketAddr, sync::Arc};

#[derive(Clone)]
pub struct RandomNode(pub Arc<Node>);

impl RandomNode {
    #[allow(dead_code)]
    pub async fn new<T: AsRef<str>>(name: T) -> Self {
        let mut config = NodeConfig::default();
        config.name = Some(name.as_ref().into());
        Self(Node::new(Some(config)).await.unwrap())
    }

    #[allow(dead_code)]
    pub async fn send_direct_message_with_len(&self, target: SocketAddr, message: &[u8]) {
        // prepend the message with its length in LE u16
        let mut bytes = Vec::with_capacity(2 + message.len());
        let u16_len = (message.len() as u16).to_le_bytes();
        bytes.extend_from_slice(&u16_len);
        bytes.extend_from_slice(message);

        self.node()
            .send_direct_message(target, bytes.into())
            .await
            .unwrap();
    }
}

impl ContainsNode for RandomNode {
    fn node(&self) -> &Arc<Node> {
        &self.0
    }
}

#[macro_export]
macro_rules! impl_messaging {
    ($target: ty) => {
        #[async_trait::async_trait]
        impl Messaging for $target {
            type Message = ();

            fn read_message(buffer: &[u8]) -> Option<&[u8]> {
                // expecting the test messages to be prefixed with their length encoded as a LE u16
                if buffer.len() >= 2 {
                    let payload_len = u16::from_le_bytes(buffer[..2].try_into().unwrap()) as usize;

                    if buffer[2..].len() >= payload_len {
                        Some(&buffer[..2 + payload_len])
                    } else {
                        None
                    }
                } else {
                    None
                }
            }

            fn parse_message(&self, _source: SocketAddr, _message: &[u8]) -> Option<Self::Message> {
                Some(())
            }

            fn process_message(&self, source: SocketAddr, _message: &Self::Message) {
                info!(parent: self.node().span(), "received a message from {}", source);
            }
        }
    };
}

impl_messaging!(RandomNode);
