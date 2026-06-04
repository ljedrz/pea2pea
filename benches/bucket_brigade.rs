//! "Bucket Brigade" per-hop latency.
//!
//! A message is handed to node 0 of a linear chain and forwarded hop-by-hop to
//! the far end. The thing worth measuring is one end-to-end traversal.
//!
//! Reading the output: divan's time column is the **end-to-end** traversal. The
//! `ItemsCount` counter is set to the hop count, so the throughput column reads
//! as hops/second - i.e. per-hop latency is its inverse (1 Mhop/s == 1 µs/hop).
//!
//! NOTE ON ULIMIT: every node holds a listener plus its chain connections, so a
//! 10k chain needs tens of thousands of file descriptors. Raise `ulimit -n`
//! (e.g. `ulimit -n 65535`) before running the large sizes, or trim the top of
//! the `args` array for a no-tuning run.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use bytes::{Bytes, BytesMut};
use divan::{Bencher, counter::ItemsCount};
use pea2pea::{
    Config, ConnectionSide, Node, Pea2Pea, Topology, connect_nodes,
    protocols::{Reading, Writing},
};
use tokio::{runtime::Runtime, sync::Notify, time::sleep};

#[path = "../tests/common/mod.rs"]
mod common;

fn main() {
    divan::main();
}

const PAYLOAD: &[u8] = b"water";

#[derive(Clone)]
struct Brigadier {
    node: Node,
    // only the last node ever fires this, to signal a traversal completed
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
        // forward to the one neighbor that isn't where this came from
        let next = self
            .node
            .connected_addrs()
            .into_iter()
            .find(|&addr| addr != source);
        match next {
            // middle of the chain: pass it along, fire-and-forget
            Some(next_hop) => {
                let _ = self.unicast_fast(next_hop, message.freeze());
            }
            // no other neighbor => we're the tail; the bucket has arrived
            None => self.finished.notify_one(),
        }
    }
}

/// One message, node 0 to the tail, swept over chain length.
///
/// Because each sample waits for the message to reach the tail before returning,
/// there is never more than one bucket in flight, which is what keeps successive
/// samples from bleeding into each other (so no per-round cooldown is needed
/// inside the clock).
#[divan::bench(sample_count = 50, sample_size = 1, args = [10, 100, 1000, 10_000])]
fn nodes(bencher: Bencher, len: usize) {
    assert!(len >= 2, "a chain needs at least one hop");

    let rt = runtime();
    let (nodes, finished, first_hop) = rt.block_on(build_chain(len));

    bencher
        .counter(ItemsCount::new(len - 1)) // hops; throughput column => hops/sec
        .bench_local(|| {
            rt.block_on(async {
                nodes[0]
                    .unicast_fast(first_hop, Bytes::from_static(PAYLOAD))
                    .unwrap();
                finished.notified().await;
            });
        });

    rt.block_on(async {
        for n in &nodes {
            n.node().shut_down().await;
        }
    });
}

/// Builds a `len`-node line, lets the topology settle, and returns the nodes,
/// the shared completion signal, and node 0's only neighbor (its first hop).
async fn build_chain(len: usize) -> (Vec<Brigadier>, Arc<Notify>, SocketAddr) {
    let finished = Arc::new(Notify::new());
    let mut nodes = Vec::with_capacity(len);

    for i in 0..len {
        let node = Brigadier {
            node: Node::new(Config {
                name: Some(format!("node_{i}")),
                ..Default::default()
            }),
            finished: finished.clone(),
        };
        node.enable_reading().await;
        node.enable_writing().await;
        node.node()
            .toggle_listener()
            .await
            .inspect_err(check_for_emfile)
            .unwrap();
        nodes.push(node);
    }

    connect_nodes(&nodes, Topology::Line)
        .await
        .inspect_err(check_for_emfile)
        .unwrap();

    // let every connection finish establishing before the first traversal
    sleep(Duration::from_millis(500)).await;

    let first_hop = nodes[0].node().connected_addrs()[0];
    (nodes, finished, first_hop)
}

/// errno 24 is EMFILE - the process ran out of file descriptors. Surfaces the
/// fix instead of letting a bare `unwrap` panic look like a library bug.
fn check_for_emfile(e: &std::io::Error) {
    if e.raw_os_error() == Some(24) {
        eprintln!("hit the open-file limit (EMFILE) - raise `ulimit -n` for large chains");
    }
}

fn runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}
