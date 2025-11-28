//! Every node connects to every other node and broadcasts a message to everyone.
//! 25 nodes = 600 connections (safe for default ulimit).
//! 50 nodes = 2,450 connections (requires increasing ulimit).

mod common;

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};

use bytes::{Bytes, BytesMut};
use pea2pea::{
    Config, ConnectionSide, Node, Pea2Pea, Topology, connect_nodes,
    protocols::{Reading, Writing},
};
use tokio::sync::Notify;
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

const NUM_NODES: usize = 25;

#[derive(Clone)]
struct DenseNode {
    node: Node,
    // shared global counter for the test
    total_received: Arc<AtomicUsize>,
    // signal to fire when target is reached
    completion_signal: Arc<Notify>,
    target_count: usize,
}

impl Pea2Pea for DenseNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Reading for DenseNode {
    type Message = BytesMut;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, _source: SocketAddr, _message: Self::Message) {
        // increment global counter
        let prev = self.total_received.fetch_add(1, Ordering::Relaxed);

        // if this was the last message, wake up the main thread
        if prev + 1 == self.target_count {
            self.completion_signal.notify_one();
        }
    }
}

impl Writing for DenseNode {
    type Message = Bytes;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::INFO);

    // calculate topology stats
    let expected_connections = NUM_NODES * (NUM_NODES - 1);
    // each node sends 1 message to every peer
    let total_expected_msgs = expected_connections;

    info!("Spawning {NUM_NODES} nodes to form a dense mesh.");
    info!(
        "Target: {} total TCP connections.",
        expected_connections / 2
    );

    // prepare shared state
    let total_received = Arc::new(AtomicUsize::new(0));
    let completion_signal = Arc::new(Notify::new());

    // spawn nodes
    let mut nodes = Vec::with_capacity(NUM_NODES);
    for i in 0..NUM_NODES {
        let config = Config {
            name: Some(format!("node_{i}")),
            max_connections: NUM_NODES as u16 + 10,
            listener_addr: Some("127.0.0.1:0".parse().unwrap()),
            ..Default::default()
        };

        let node = DenseNode {
            node: Node::new(config),
            total_received: total_received.clone(),
            completion_signal: completion_signal.clone(),
            target_count: total_expected_msgs,
        };

        node.enable_reading().await;
        node.enable_writing().await;
        node.node().toggle_listener().await.unwrap();

        nodes.push(node);
    }

    // connect
    info!("Connecting nodes...");
    let start_conn = Instant::now();
    connect_nodes(&nodes, Topology::Mesh).await.unwrap();
    let conn_duration = start_conn.elapsed();

    info!("Mesh established in {conn_duration:.2?}.");

    // broadcast
    info!("Broadcasting messages...");
    let payload = Bytes::from(&b"hello mesh"[..]);
    let start_broadcast = Instant::now();

    for node in &nodes {
        node.broadcast(payload.clone()).unwrap();
    }

    // wait for completion
    completion_signal.notified().await;

    let duration = start_broadcast.elapsed();
    let count = total_received.load(Ordering::Relaxed);

    info!("Success! {count} messages exchanged in {duration:.2?}.");
    info!(
        "Throughput: {:.0} msgs/sec",
        count as f64 / duration.as_secs_f64()
    );
}
