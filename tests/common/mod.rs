#![allow(dead_code)]

use std::{
    io,
    marker::PhantomData,
    net::SocketAddr,
    sync::{Arc, OnceLock},
    time::Duration,
};

use bytes::{Bytes, BytesMut};
use pea2pea::{Config, Node, Pea2Pea, Topology, connect_nodes, protocols::*};
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

/// A helper trait to shorten the calls to `Writing::unicast` in tests.
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
        common::TestNode {
            node: pea2pea::Node::new(config),
            barrier: Default::default(),
        }
    }};
}

pub async fn start_test_nodes(count: usize) -> Vec<TestNode> {
    let mut nodes = Vec::with_capacity(count);

    for _ in 0..count {
        let test_node = TestNode {
            node: Node::new(Default::default()),
            barrier: Default::default(),
        };
        test_node.node().toggle_listener().await.unwrap();
        nodes.push(test_node);
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
            async fn on_disconnect(&self, _addr: SocketAddr) {}
        }
    };
}

impl OnConnect for TestNode {
    async fn on_connect(&self, _addr: SocketAddr) {
        if let Some(barrier) = self.barrier.get() {
            barrier.wait().await;
        }
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
        format!("{bytes:.2} B")
    }
}

pub async fn wait_until<F>(within: Duration, mut cond: F)
where
    F: FnMut() -> bool,
{
    let res = tokio::time::timeout(within, async {
        let mut interval = tokio::time::interval(Duration::from_millis(10));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if cond() {
                return;
            }
        }
    })
    .await;
    assert!(res.is_ok(), "condition not satisfied within {within:?}");
}

pub fn named_node(name: &str) -> Node {
    Node::new(Config {
        name: Some(name.into()),
        ..Default::default()
    })
}

pub async fn start_listening<T: Pea2Pea>(node: &T) -> SocketAddr {
    node.node().toggle_listener().await.unwrap().unwrap()
}

pub async fn wait_for_connections<T: Pea2Pea>(node: &T, n: usize) {
    wait_until(Duration::from_secs(1), || node.node().num_connected() == n).await;
}

pub async fn connect_and_wait(nodes: &[TestNode], topology: Topology) -> io::Result<()> {
    // each edge triggers OnConnect on both peers, +1 for our wait below
    let expected_fires = topology.num_expected_connections(nodes.len()) + 1;
    let barrier = Arc::new(Barrier::new(expected_fires));
    for node in nodes {
        node.barrier
            .set(barrier.clone())
            .expect("barrier already configured");
        node.enable_on_connect().await;
    }
    connect_nodes(nodes, topology).await?;
    barrier.wait().await;
    Ok(())
}
