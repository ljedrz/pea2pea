//! A dense mesh stress test that measures RAM usage per node and per connection.
//!
//! This example spawns a large number of nodes and connects them in a full mesh
//! topology, reporting the memory footprint to demonstrate the library's
//! lightweight architecture.
//!
//! note: To run this with >50 nodes, you likely need to increase your open file limit
//! by running `ulimit -n 10000` (or more).

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
use peak_alloc::PeakAlloc;
use tokio::sync::Notify;

// use the `peak_alloc` global allocator to track heap use
#[global_allocator]
static PEAK_ALLOC: PeakAlloc = PeakAlloc;

// be mindful of your `ulimit`
const NUM_NODES: usize = 200; // 39800 connections

#[derive(Clone)]
struct MeshNode {
    node: Node,
    // global counter for messages received across the entire mesh
    total_received: Arc<AtomicUsize>,
    // signal to fire when target count is reached
    completion_signal: Arc<Notify>,
    target_count: usize,
}

impl Pea2Pea for MeshNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Reading for MeshNode {
    type Message = BytesMut;
    type Codec = common::TestCodec<Self::Message>;

    // optimization: reduce buffer from default 64kB to 4kB to minimize footprint
    const INITIAL_BUFFER_SIZE: usize = 4 * 1024;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, _source: SocketAddr, _message: Self::Message) {
        let prev = self.total_received.fetch_add(1, Ordering::Relaxed);
        if prev + 1 == self.target_count {
            self.completion_signal.notify_one();
        }
    }
}

impl Writing for MeshNode {
    type Message = Bytes;
    type Codec = common::TestCodec<Self::Message>;

    // optimization: reduce buffer from default 64kB to 4kB to minimize footprint
    const INITIAL_BUFFER_SIZE: usize = 4 * 1024;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

#[tokio::main]
async fn main() {
    let initial_mem = PEAK_ALLOC.current_usage();
    println!("--- Dense Mesh Benchmark (RAM & Throughput) ---");
    println!("Initial Heap Usage: {:.2} MB", to_mb(initial_mem));

    let expected_connections = NUM_NODES * (NUM_NODES - 1);
    let total_expected_msgs = expected_connections;
    let total_received = Arc::new(AtomicUsize::new(0));
    let completion_signal = Arc::new(Notify::new());

    // spawn nodes
    println!("\n[1/3] Spawning {NUM_NODES} nodes...");
    let start_spawn = Instant::now();

    let mut nodes = Vec::with_capacity(NUM_NODES);
    for i in 0..NUM_NODES {
        let config = Config {
            name: Some(format!("node_{i}")),
            max_connections: NUM_NODES as u16 + 10,
            max_connections_per_ip: NUM_NODES as u16 + 10,
            max_connecting: NUM_NODES as u16 + 10,
            listener_addr: Some("127.0.0.1:0".parse().unwrap()),
            ..Default::default()
        };

        let node = MeshNode {
            node: Node::new(config),
            total_received: total_received.clone(),
            completion_signal: completion_signal.clone(),
            target_count: total_expected_msgs,
        };

        node.enable_reading().await;
        node.enable_writing().await;
        node.node()
            .toggle_listener()
            .await
            .inspect_err(common::check_for_24)
            .unwrap();

        nodes.push(node);
    }

    let spawn_duration = start_spawn.elapsed();
    let mem_after_spawn = PEAK_ALLOC.current_usage();
    let nodes_mem_usage = mem_after_spawn - initial_mem;

    println!("      Spawned in:     {:.2?}", spawn_duration);
    println!(
        "      Heap Usage:     {:.2} MB (+{:.2} MB)",
        to_mb(mem_after_spawn),
        to_mb(nodes_mem_usage)
    );
    println!(
        "      Avg per Node:   {:.2} kB",
        (nodes_mem_usage as f64 / NUM_NODES as f64) / 1024.0
    );

    // connect mesh
    println!("\n[2/3] Establishing Full Mesh ({expected_connections} connections)...");
    let start_conn = Instant::now();

    connect_nodes(&nodes, Topology::Mesh)
        .await
        .inspect_err(common::check_for_24)
        .unwrap();

    let conn_duration = start_conn.elapsed();
    let mem_after_mesh = PEAK_ALLOC.current_usage();
    let mesh_mem_usage = mem_after_mesh - mem_after_spawn;

    println!("      Connected in:   {:.2?}", conn_duration);
    println!(
        "      Heap Usage:     {:.2} MB (+{:.2} MB)",
        to_mb(mem_after_mesh),
        to_mb(mesh_mem_usage)
    );
    println!(
        "      Avg per Conn:   {:.2} kB",
        (mesh_mem_usage as f64 / expected_connections as f64) / 1024.0
    );

    // broadcast traffic
    println!("\n[3/3] Broadcasting messages (stress test)...");
    let payload = Bytes::from(vec![0u8; 1024]); // 1KB payload
    let start_broadcast = Instant::now();

    for node in &nodes {
        node.broadcast(payload.clone()).unwrap();
    }

    completion_signal.notified().await;

    let duration = start_broadcast.elapsed();
    let count = total_received.load(Ordering::Relaxed);
    let throughput = count as f64 / duration.as_secs_f64();

    println!("      Traffic:        {count} messages delivered");
    println!("      Duration:       {:.2?}", duration);
    println!("      Throughput:     {:.0} msgs/sec", throughput);

    // Clean up
    println!("\n--- Shutting Down ---");
    for n in nodes {
        n.node().shut_down().await;
    }
}

fn to_mb(bytes: usize) -> f64 {
    bytes as f64 / 1_000_000.0
}
