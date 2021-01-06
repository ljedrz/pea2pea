use crate::Pea2Pea;

use std::{
    collections::HashSet,
    io::{self, ErrorKind},
};

/// The way in which nodes are connected to each other; used in `connect_nodes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topology {
    /// Each node - except the last one - connects to the next one in a linear fashion.
    Line,
    /// Like the `Line`, but the last node connects to the first one, forming a ring.
    Ring,
    /// All the nodes become connected to one another, forming a full mesh.
    Mesh,
    /// The first node is the central one (the hub); all the other nodes connect to it.
    Star,
}

/// Connects the provided list of nodes in order to form the given `Topology`.
pub async fn connect_nodes<T: Pea2Pea>(nodes: &[T], topology: Topology) -> io::Result<()> {
    if nodes.len() < 2 {
        // there must be more than one node in order to have any connections
        return Err(ErrorKind::Other.into());
    }

    let count = nodes.len();

    match topology {
        Topology::Line | Topology::Ring => {
            for i in 0..(count - 1) {
                nodes[i]
                    .node()
                    .connect(nodes[i + 1].node().listening_addr())
                    .await?;
            }
            if topology == Topology::Ring {
                nodes[count - 1]
                    .node()
                    .connect(nodes[0].node().listening_addr())
                    .await?;
            }
        }
        Topology::Mesh => {
            let mut connected_pairs = HashSet::with_capacity((count - 1) * 2);
            for i in 0..count {
                for (j, peer) in nodes.iter().enumerate() {
                    if i != j && connected_pairs.insert((i, j)) && connected_pairs.insert((j, i)) {
                        nodes[i]
                            .node()
                            .connect(peer.node().listening_addr())
                            .await?;
                    }
                }
            }
        }
        Topology::Star => {
            let hub_addr = nodes[0].node().listening_addr();
            for node in nodes.iter().skip(1) {
                node.node().connect(hub_addr).await?;
            }
        }
    }

    Ok(())
}
