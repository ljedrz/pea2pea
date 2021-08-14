#![allow(dead_code)]

use bytes::Bytes;
use tracing::*;

use pea2pea::{
    protocols::{Reading, Writing},
    Config, Node, Pea2Pea,
};

use std::{
    convert::TryInto,
    io::{self, Read},
    net::SocketAddr,
};

pub async fn start_nodes(count: usize, config: Option<Config>) -> Vec<Node> {
    let mut nodes = Vec::with_capacity(count);

    for _ in 0..count {
        let node = Node::new(config.clone()).await.unwrap();
        nodes.push(node);
    }

    nodes
}

#[derive(Clone)]
pub struct InertNode(pub Node);

impl Pea2Pea for InertNode {
    fn node(&self) -> &Node {
        &self.0
    }
}

impl std::ops::Deref for InertNode {
    type Target = Node;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub async fn start_inert_nodes(count: usize, config: Option<Config>) -> Vec<InertNode> {
    start_nodes(count, config)
        .await
        .into_iter()
        .map(InertNode)
        .collect()
}

#[derive(Clone)]
pub struct MessagingNode(pub Node);

impl MessagingNode {
    pub async fn new<T: Into<String>>(name: T) -> Self {
        let config = Config {
            name: Some(name.into()),
            ..Default::default()
        };
        Self(Node::new(Some(config)).await.unwrap())
    }
}

impl Pea2Pea for MessagingNode {
    fn node(&self) -> &Node {
        &self.0
    }
}

pub fn read_len_prefixed_message<R: io::Read, const N: usize>(
    reader: &mut R,
) -> io::Result<Option<Vec<u8>>> {
    let mut len_arr = [0u8; N];
    if reader.read_exact(&mut len_arr).is_err() {
        return Ok(None);
    }
    let payload_len = match N {
        2 => u16::from_le_bytes(len_arr[..].try_into().unwrap()) as usize,
        4 => u32::from_le_bytes(len_arr[..].try_into().unwrap()) as usize,
        _ => unreachable!(),
    };

    if payload_len == 0 {
        return Err(io::ErrorKind::InvalidData.into());
    }

    let mut buffer = vec![0u8; payload_len];
    if reader
        .take(payload_len as u64)
        .read_exact(&mut buffer)
        .is_err()
    {
        Ok(None)
    } else {
        Ok(Some(buffer))
    }
}

pub fn prefix_with_len(len_size: usize, message: &[u8]) -> Bytes {
    let mut vec = Vec::with_capacity(len_size + message.len());

    match len_size {
        2 => vec.extend_from_slice(&(message.len() as u16).to_le_bytes()),
        4 => vec.extend_from_slice(&(message.len() as u32).to_le_bytes()),
        _ => unreachable!(),
    }

    vec.extend_from_slice(message);

    vec.into()
}

#[macro_export]
macro_rules! impl_messaging {
    ($target: ty) => {
        #[async_trait::async_trait]
        impl Reading for $target {
            type Message = Bytes;

            fn read_message<R: io::Read>(&self, _source: SocketAddr, reader: &mut R) -> io::Result<Option<Self::Message>> {
                let vec = crate::common::read_len_prefixed_message::<R, 4>(reader)?;

                Ok(vec.map(Bytes::from))
            }

            async fn process_message(&self, source: SocketAddr, _message: Self::Message) -> io::Result<()> {
                info!(parent: self.node().span(), "received a message from {}", source);

                Ok(())
            }
        }

        impl Writing for $target {
            fn write_message<W: io::Write>(&self, _target: SocketAddr, payload: &[u8], writer: &mut W) -> io::Result<()> {
                writer.write_all(&(payload.len() as u32).to_le_bytes())?;
                writer.write_all(&payload)
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
