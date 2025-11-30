use std::{collections::HashSet, io};

use crate::Pea2Pea;

/// The way in which nodes are connected to each other; used in [`connect_nodes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topology {
    /// Each node - except the last one - connects to the next one in a linear fashion.
    Line,
    /// Like [`Topology::Line`], but the last node connects to the first one, forming a ring.
    Ring,
    /// All the nodes become connected to one another, forming a full mesh.
    Mesh,
    /// The first node is the central one (the hub); all the other nodes connect to it.
    Star,
}

impl Topology {
    /// Returns the expected total number of connections for the given number of nodes.
    pub fn num_expected_connections(&self, num_nodes: usize) -> usize {
        if num_nodes == 0 {
            return 0;
        }

        match self {
            Self::Line => (num_nodes - 1) * 2,
            Self::Ring => num_nodes * 2,
            Self::Mesh => (num_nodes - 1) * num_nodes,
            Self::Star => (num_nodes - 1) * 2,
        }
    }
}

/// Connects the provided list of nodes in order to form the given [`Topology`].
pub async fn connect_nodes<T: Pea2Pea>(nodes: &[T], topology: Topology) -> io::Result<()> {
    let count = nodes.len();
    if count < 2 {
        // there must be more than one node in order to have any connections
        return Err(io::ErrorKind::InvalidInput.into());
    }

    match topology {
        Topology::Line | Topology::Ring => {
            for i in 0..(count - 1) {
                let addr = nodes[i + 1].node().listening_addr().await?;
                nodes[i].node().connect(addr).await?;
            }
            if topology == Topology::Ring {
                let addr = nodes[0].node().listening_addr().await?;
                nodes[count - 1].node().connect(addr).await?;
            }
        }
        Topology::Mesh => {
            let mut connected_pairs = HashSet::with_capacity((count - 1) * 2);
            for i in 0..count {
                for (j, peer) in nodes.iter().enumerate() {
                    if i != j && connected_pairs.insert((i, j)) && connected_pairs.insert((j, i)) {
                        let addr = peer.node().listening_addr().await?;
                        nodes[i].node().connect(addr).await?;
                    }
                }
            }
        }
        Topology::Star => {
            let hub_addr = nodes[0].node().listening_addr().await?;
            for node in nodes.iter().skip(1) {
                node.node().connect(hub_addr).await?;
            }
        }
    }

    Ok(())
}
