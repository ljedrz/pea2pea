use std::{collections::HashSet, io};

use crate::Pea2Pea;

/// The way in which nodes are connected to each other; used in [`connect_nodes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Topology {
    /// Each node - except the last one - connects to the next one in a linear fashion.
    Line,
    /// Like [`Topology::Line`], but the last node connects to the first one, forming a ring.
    Ring,
    /// All the nodes become connected to one another, forming a full mesh.
    Mesh,
    /// The first node is the central one (the hub); all the other nodes connect to it.
    Star,
    /// Nodes are connected in a 2D grid lattice.
    /// The number of nodes must equal `width * height`.
    Grid {
        /// The width of the grid.
        width: usize,
        /// The height of the grid.
        height: usize,
    },
    /// The relationships between the nodes form a binary tree.
    Tree,
    /// Each node connects to a specific number of unique random peers.
    /// Uses a deterministic hash seeded by `seed`.
    Random {
        /// The number of connections each node attempts to initiate.
        degree: usize,
        /// A randomness seed for reproducibility.
        seed: u64,
    },
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
            Self::Grid { width, height } => ((width * height) * 2 - width - height) * 2,
            Self::Tree => (num_nodes - 1) * 2,
            Self::Random { degree, seed: _ } => num_nodes * degree * 2,
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
        Topology::Grid { width, height } => {
            if width * height != count {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "Grid topology dimensions ({width}x{height} = {}) do not match the number of nodes ({count})",
                        width * height
                    ),
                ));
            }

            for row in 0..height {
                for col in 0..width {
                    let i = row * width + col;

                    // connect right
                    if col + 1 < width {
                        let target = i + 1;
                        let addr = nodes[target].node().listening_addr().await?;
                        nodes[i].node().connect(addr).await?;
                    }

                    // connect down
                    if row + 1 < height {
                        let target = i + width;
                        let addr = nodes[target].node().listening_addr().await?;
                        nodes[i].node().connect(addr).await?;
                    }
                }
            }
        }
        Topology::Tree => {
            for i in 0..count {
                let left = 2 * i + 1;
                if left < count {
                    let addr = nodes[left].node().listening_addr().await?;
                    nodes[i].node().connect(addr).await?;
                }
                let right = 2 * i + 2;
                if right < count {
                    let addr = nodes[right].node().listening_addr().await?;
                    nodes[i].node().connect(addr).await?;
                }
            }
        }
        Topology::Random { degree, seed } => {
            if degree >= count {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Random topology degree cannot exceed N-1",
                ));
            }

            for i in 0..count {
                let mut chosen_targets = HashSet::with_capacity(degree);
                let mut attempt = 0u64;

                // simple loop: keep picking targets until we satisfy the degree
                while chosen_targets.len() < degree {
                    attempt += 1;

                    // stupidly simple deterministic mixer
                    let mut x = (i as u64).wrapping_add(seed).wrapping_add(attempt);
                    x = x.wrapping_mul(0x517cc1b727220a95); // large odd constant
                    x ^= x >> 32; // mix high bits

                    let target = (x as usize) % count;

                    // ensure no self-connection and no duplicate outbound connections
                    if target != i && chosen_targets.insert(target) {
                        let addr = nodes[target].node().listening_addr().await?;
                        let _ = nodes[i].node().connect(addr).await;
                    }
                }
            }
        }
    }

    Ok(())
}
