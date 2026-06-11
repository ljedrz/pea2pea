//! This module provides the BarrierNode, which is primarily used in topology tests, where no
//! messages are ever passed, and all the nodes need to be synchronized connection-wise before the
//! test can begin.

use std::{
    io,
    net::SocketAddr,
    sync::{Arc, OnceLock},
};

use pea2pea::{Config, Node, Pea2Pea, Topology, connect_nodes, protocols::OnConnect};
use tokio::sync::Barrier;

#[derive(Clone)]
pub struct BarrierNode {
    node: Node,
    barrier: OnceLock<Arc<Barrier>>,
}

impl Pea2Pea for BarrierNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Default for BarrierNode {
    fn default() -> Self {
        Self {
            node: Node::new(Config::default()),
            barrier: Default::default(),
        }
    }
}

impl OnConnect for BarrierNode {
    async fn on_connect(&self, _addr: SocketAddr) {
        if let Some(barrier) = self.barrier.get() {
            barrier.wait().await;
        }
    }
}

pub async fn connect_and_wait(nodes: &[BarrierNode], topology: Topology) -> io::Result<()> {
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
