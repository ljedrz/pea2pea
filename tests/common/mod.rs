#![allow(dead_code)]

use bytes::{Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec};
use tracing::*;

use pea2pea::{
    protocols::{Reading, Writing},
    Config, Node, Pea2Pea,
};

use std::{io, marker::PhantomData, net::SocketAddr, ops::Deref};

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

impl Deref for InertNode {
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
            initial_read_buffer_size: 256,
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

pub struct TestCodec<M>(pub LengthDelimitedCodec, PhantomData<M>);

impl Decoder for TestCodec<BytesMut> {
    type Item = BytesMut;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let ret = self.0.decode(src)?;

        if let Some(ref msg) = ret {
            if msg.is_empty() {
                return Err(io::ErrorKind::InvalidData.into());
            }
        }

        Ok(ret)
    }
}

impl<M> Encoder<Bytes> for TestCodec<M> {
    type Error = io::Error;

    fn encode(&mut self, item: Bytes, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.0.encode(item, dst)
    }
}

impl<M> Default for TestCodec<M> {
    fn default() -> Self {
        let inner = LengthDelimitedCodec::builder()
            .length_field_length(2)
            .little_endian()
            .new_codec();
        Self(inner, PhantomData)
    }
}

pub fn display_bytes(bytes: f64) -> String {
    const GB: f64 = 1_000_000_000.0;
    const MB: f64 = 1_000_000.0;
    const KB: f64 = 1_000.0;

    if bytes >= GB {
        format!("{:.2} GB", bytes / GB)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{:.2} kB", bytes / KB)
    } else {
        format!("{:.2} B", bytes)
    }
}

#[macro_export]
macro_rules! impl_messaging {
    ($target: ty) => {
        #[async_trait::async_trait]
        impl Reading for $target {
            type Message = bytes::BytesMut;
            type Codec = crate::common::TestCodec<Self::Message>;

            fn codec(&self, _addr: SocketAddr) -> Self::Codec {
                Default::default()
            }

            async fn process_message(&self, source: SocketAddr, _message: Self::Message) -> io::Result<()> {
                info!(parent: self.node().span(), "received a message from {}", source);

                Ok(())
            }
        }

        impl Writing for $target {
            type Message = bytes::Bytes;
            type Codec = crate::common::TestCodec<Self::Message>;

            fn codec(&self, _addr: SocketAddr) -> Self::Codec {
                Default::default()
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
            assert!(
                now.elapsed() <= std::time::Duration::from_secs($limit_secs),
                "timed out!"
            );
        }
    };
}
