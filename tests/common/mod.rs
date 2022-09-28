#![allow(dead_code)]

use std::{io, marker::PhantomData, net::SocketAddr};

use bytes::{Bytes, BytesMut};
use pea2pea::{
    protocols::{Reading, Writing},
    Node, Pea2Pea,
};
use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec};
use tracing::*;

#[derive(Clone)]
pub struct TestNode(pub Node);

impl Pea2Pea for TestNode {
    fn node(&self) -> &Node {
        &self.0
    }
}

/// A helper trait to shorten the calls to `Writing::unicast` in tests.
#[async_trait::async_trait]
pub trait WritingExt: Writing {
    async fn send_dm(&self, addr: SocketAddr, msg: <Self as Writing>::Message) -> io::Result<()> {
        self.unicast(addr, msg)?.await.unwrap()
    }
}

impl<T: Writing> WritingExt for T {}

#[macro_export]
macro_rules! test_node {
    ($name: expr) => {{
        let config = pea2pea::Config {
            name: Some($name.into()),
            ..Default::default()
        };
        common::TestNode(pea2pea::Node::new(config).await.unwrap())
    }};
}

pub async fn start_test_nodes(count: usize) -> Vec<TestNode> {
    let mut nodes = Vec::with_capacity(count);

    for _ in 0..count {
        nodes.push(TestNode(Node::new(Default::default()).await.unwrap()));
    }

    nodes
}

pub struct TestCodec<M>(pub LengthDelimitedCodec, PhantomData<M>);

impl<M> Default for TestCodec<M> {
    fn default() -> Self {
        let inner = LengthDelimitedCodec::builder()
            .length_field_length(2)
            .new_codec();
        Self(inner, PhantomData)
    }
}

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

#[macro_export]
macro_rules! impl_messaging {
    ($target: ty) => {
        #[async_trait::async_trait]
        impl Reading for $target {
            type Message = bytes::BytesMut;
            type Codec = $crate::common::TestCodec<Self::Message>;

            fn codec(&self, _addr: SocketAddr, _side: pea2pea::ConnectionSide) -> Self::Codec {
                Default::default()
            }

            async fn process_message(&self, source: SocketAddr, _message: Self::Message) -> io::Result<()> {
                info!(parent: self.node().span(), "received a message from {}", source);

                Ok(())
            }
        }

        impl Writing for $target {
            type Message = bytes::Bytes;
            type Codec = $crate::common::TestCodec<Self::Message>;

            fn codec(&self, _addr: SocketAddr, _side: pea2pea::ConnectionSide) -> Self::Codec {
                Default::default()
            }
        }
    };
}

impl_messaging!(TestNode);

#[macro_export]
macro_rules! impl_noop_disconnect_and_handshake {
    ($target: ty) => {
        #[async_trait::async_trait]
        impl Handshake for $target {
            async fn perform_handshake(
                &self,
                conn: pea2pea::Connection,
            ) -> io::Result<pea2pea::Connection> {
                Ok(conn)
            }
        }

        #[async_trait::async_trait]
        impl Disconnect for $target {
            async fn handle_disconnect(&self, _addr: SocketAddr) {}
        }
    };
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
