//! This module contains the FullNoopNode, which implements all the Pea2Pea protocols,
//! none of which do anything in particular. It is useful for tests or benchmarks where
//! the protocol logic is unimportant - only their plumbing. It technically uses a real
//! codec, it just discards all the messages it receives; it can be used to send messages,
//! too.

use std::{io, net::SocketAddr};

use pea2pea::{
    Config, ConnectionSide, DisconnectOrigin, Node, Pea2Pea,
    protocols::{Handshake, OnConnect, OnDisconnect, Reading, Writing},
};
use tokio_util::codec::BytesCodec;

#[derive(Clone)]
pub struct FullNoopNode(pub Node);

impl Default for FullNoopNode {
    fn default() -> Self {
        Self(Node::new(Config::default()))
    }
}

impl Pea2Pea for FullNoopNode {
    fn node(&self) -> &Node {
        &self.0
    }
}

impl Handshake for FullNoopNode {
    async fn perform_handshake(
        &self,
        conn: pea2pea::Connection,
    ) -> io::Result<pea2pea::Connection> {
        Ok(conn)
    }
}

impl OnConnect for FullNoopNode {
    async fn on_connect(&self, _addr: SocketAddr) {}
}

impl OnDisconnect for FullNoopNode {
    async fn on_disconnect(&self, _addr: SocketAddr, _origin: DisconnectOrigin) {}
}

impl Reading for FullNoopNode {
    type Message = bytes::BytesMut;
    type Codec = BytesCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, _source: SocketAddr, _message: Self::Message) {}
}

impl Writing for FullNoopNode {
    type Message = bytes::Bytes;
    type Codec = BytesCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}
