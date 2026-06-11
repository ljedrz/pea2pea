#![allow(dead_code)]

use std::{
    io,
    marker::PhantomData,
    net::SocketAddr,
    sync::{Arc, OnceLock},
};

use bytes::{Bytes, BytesMut};
use pea2pea::{Config, Node, Pea2Pea, protocols::*};
use tokio::sync::Barrier;
use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec};

#[derive(Clone)]
pub struct TestNode {
    pub node: Node,
    pub barrier: OnceLock<Arc<Barrier>>,
}

impl Pea2Pea for TestNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

#[macro_export]
macro_rules! test_node {
    ($name: expr) => {{
        let config = pea2pea::Config {
            name: Some($name.into()),
            ..Default::default()
        };
        common::TestNode {
            node: pea2pea::Node::new(config),
            barrier: Default::default(),
        }
    }};
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

        if let Some(ref msg) = ret
            && msg.is_empty()
        {
            return Err(io::ErrorKind::InvalidData.into());
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
        impl Reading for $target {
            type Message = bytes::BytesMut;
            type Codec = $crate::common::TestCodec<Self::Message>;

            fn codec(&self, _addr: SocketAddr, _side: pea2pea::ConnectionSide) -> Self::Codec {
                Default::default()
            }

            async fn process_message(&self, _source: SocketAddr, _message: Self::Message) {}
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
        impl Handshake for $target {
            async fn perform_handshake(
                &self,
                conn: pea2pea::Connection,
            ) -> io::Result<pea2pea::Connection> {
                Ok(conn)
            }
        }

        impl OnDisconnect for $target {
            async fn on_disconnect(
                &self,
                _addr: SocketAddr,
                _origin: pea2pea::connections::DisconnectOrigin,
            ) {
            }
        }
    };
}

pub fn named_node(name: &str) -> Node {
    Node::new(Config {
        name: Some(name.into()),
        ..Default::default()
    })
}
