#![allow(dead_code)]

use bytes::Bytes;
use tracing::*;

use pea2pea::{Messaging, Node, NodeConfig, Pea2Pea};

use std::{convert::TryInto, io, net::SocketAddr, sync::Arc};

pub async fn start_nodes(count: usize, config: Option<NodeConfig>) -> Vec<Arc<Node>> {
    let mut nodes = Vec::with_capacity(count);

    for _ in 0..count {
        let node = Node::new(config.clone()).await.unwrap();
        nodes.push(node);
    }

    nodes
}

#[derive(Clone)]
pub struct InertNode(pub Arc<Node>);

impl Pea2Pea for InertNode {
    fn node(&self) -> &Arc<Node> {
        &self.0
    }
}

impl std::ops::Deref for InertNode {
    type Target = Arc<Node>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub async fn start_inert_nodes(count: usize, config: Option<NodeConfig>) -> Vec<InertNode> {
    start_nodes(count, config)
        .await
        .into_iter()
        .map(InertNode)
        .collect()
}

#[derive(Clone)]
pub struct MessagingNode(pub Arc<Node>);

impl MessagingNode {
    pub async fn new<T: AsRef<str>>(name: T) -> Self {
        let mut config = NodeConfig::default();
        config.name = Some(name.as_ref().into());
        Self(Node::new(Some(config)).await.unwrap())
    }
}

impl Pea2Pea for MessagingNode {
    fn node(&self) -> &Arc<Node> {
        &self.0
    }
}

pub fn read_len_prefixed_message(len_size: usize, buffer: &[u8]) -> io::Result<Option<&[u8]>> {
    if buffer.len() >= 2 {
        let payload_len = match len_size {
            2 => u16::from_le_bytes(buffer[..len_size].try_into().unwrap()) as usize,
            4 => u32::from_le_bytes(buffer[..len_size].try_into().unwrap()) as usize,
            _ => unimplemented!(),
        };

        if payload_len == 0 {
            return Err(io::ErrorKind::InvalidData.into());
        }

        if buffer[len_size..].len() >= payload_len {
            Ok(Some(&buffer[..len_size + payload_len]))
        } else {
            Ok(None)
        }
    } else {
        Ok(None)
    }
}

pub fn prefix_with_len(len_size: usize, message: &[u8]) -> Bytes {
    let mut bytes = Vec::with_capacity(len_size + message.len());

    match len_size {
        2 => bytes.extend_from_slice(&(message.len() as u16).to_le_bytes()),
        4 => bytes.extend_from_slice(&(message.len() as u32).to_le_bytes()),
        _ => unimplemented!(),
    }

    bytes.extend_from_slice(message);

    bytes.into()
}

#[macro_export]
macro_rules! impl_messaging {
    ($target: ty) => {
        #[async_trait::async_trait]
        impl Messaging for $target {
            type Message = Bytes;

            fn read_message(&self, _source: SocketAddr, buffer: &[u8]) -> io::Result<Option<(Self::Message, usize)>> {
                let bytes = crate::common::read_len_prefixed_message(2, buffer)?;

                Ok(bytes.map(|bytes| (Bytes::copy_from_slice(&bytes[2..]), bytes.len())))
            }

            async fn process_message(&self, source: SocketAddr, _message: Self::Message) -> io::Result<()> {
                info!(parent: self.node().span(), "received a message from {}", source);
                Ok(())
            }
        }
    };
}

impl_messaging!(MessagingNode);

#[macro_export]
macro_rules! wait_until {
    ($limit_secs: expr, $condition: expr) => {
        let now = std::time::Instant::now();
        loop {
            if $condition {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            if now.elapsed() > std::time::Duration::from_secs($limit_secs) {
                panic!("timed out!");
            }
        }
    };
}
