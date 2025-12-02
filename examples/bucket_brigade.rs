//! "The Bucket Brigade" Latency Test.
//!
//! Spawns 100 nodes in a linear chain (Line Topology).
//! Passes a message from Node 0 -> Node 99.
//!
//! Measures the "Per-Hop" latency. This demonstrates the near-zero
//! processing overhead of the library.

mod common;

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::{Bytes, BytesMut};
use pea2pea::{
    Config, ConnectionSide, Node, Pea2Pea, Topology, connect_nodes,
    protocols::{Reading, Writing},
};
use tokio::{sync::Notify, time::sleep};
use tracing_subscriber::filter::LevelFilter;

const NUM_NODES: usize = 10_000;
const ROUNDS: usize = 10;

#[derive(Clone)]
struct Brigadier {
    node: Node,
    // only used by the last node to signal completion
    finished: Arc<Notify>,
}

impl Pea2Pea for Brigadier {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Writing for Brigadier {
    type Message = Bytes;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

impl Reading for Brigadier {
    type Message = BytesMut;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) {
        // logic: forward to the neighbor that isn't the source
        let neighbors = self.node.connected_addrs();

        // find the "other" connection
        if let Some(next_hop) = neighbors.into_iter().find(|&addr| addr != source) {
            // we are in the middle of the chain. pass it on!
            // fire-and-forget unicast for maximum speed
            let _ = self.unicast(next_hop, message.freeze());
        } else {
            // We have no other neighbors; we must be the end of the line
            self.finished.notify_one();
        }
    }
}

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::WARN);

    println!("--- The Bucket Brigade ---");

    let finished = Arc::new(Notify::new());
    let mut nodes = Vec::with_capacity(NUM_NODES);

    // spawn nodes
    for i in 0..NUM_NODES {
        let node = Brigadier {
            node: Node::new(Config {
                name: Some(format!("node_{i}")),
                ..Default::default()
            }),
            finished: finished.clone(),
        };
        node.enable_reading().await;
        node.enable_writing().await;
        node.node().toggle_listener().await.unwrap();
        nodes.push(node);
    }

    // connect in a line (a-b-c-d...)
    connect_nodes(&nodes, Topology::Line).await.unwrap();

    // allow topology to stabilize
    sleep(Duration::from_millis(500)).await;

    let mut total_duration = Duration::ZERO;

    for round in 1..=ROUNDS {
        let start = Instant::now();

        // give the bucket to node 0. it has only 1 neighbor (node 1)
        let first_neighbor = nodes[0].node().connected_addrs()[0];
        nodes[0]
            .unicast(first_neighbor, Bytes::from_static(b"water"))
            .unwrap();

        // wait for it to reach node 99
        finished.notified().await;

        let elapsed = start.elapsed();
        total_duration += elapsed;

        // calculate per-hop latency
        // hops = NUM_NODES - 1
        let hops = (NUM_NODES - 1) as f64;
        let per_hop = elapsed.as_secs_f64() * 1_000_000.0 / hops;

        println!(
            "Round {}: Total {:.2?} | Per Hop: {:.2} µs",
            round, elapsed, per_hop
        );

        // small cool-down to ensure the queues drain
        sleep(Duration::from_millis(10)).await;
    }

    println!("--- Results ---");
    let avg_total = total_duration / (ROUNDS as u32);
    let avg_hop = (avg_total.as_secs_f64() * 1_000_000.0) / ((NUM_NODES - 1) as f64);

    println!("Average End-to-End: {:.2?}", avg_total);
    println!("Average Per-Hop:    {:.2} µs", avg_hop);

    // clean shutdown
    for n in nodes {
        n.node().shut_down().await;
    }
}
