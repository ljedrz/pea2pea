use crate::{Node, NodeConfig};

use std::{collections::HashSet, io, sync::Arc};

/// The way in which nodes are connected to each other; to be used with spawn_nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topology {
    /// no connections
    None,
    /// a > b > c
    Line,
    /// a > b > c > a
    Ring,
    /// a <> b <> c <> a
    Mesh,
    /// a > b, a > c
    Star,
}

pub async fn spawn_nodes(count: usize, topology: Topology) -> io::Result<Vec<Arc<Node>>> {
    let mut nodes = Vec::with_capacity(count);

    for i in 0..count {
        let mut config = NodeConfig::default();
        config.name = Some(i.to_string());
        let node = Node::new(Some(config)).await?;
        nodes.push(node);
    }

    match topology {
        Topology::Line | Topology::Ring => {
            for i in 0..(count - 1) {
                nodes[i]
                    .initiate_connection(nodes[i + 1].local_addr)
                    .await?;
            }
            if topology == Topology::Ring {
                nodes[count - 1]
                    .initiate_connection(nodes[0].local_addr)
                    .await?;
            }
        }
        Topology::Mesh => {
            let mut connected_pairs = HashSet::with_capacity((count - 1) * 2);
            for i in 0..count {
                for (j, peer) in nodes.iter().enumerate() {
                    if i != j {
                        if connected_pairs.insert((i, j)) && connected_pairs.insert((j, i)) {
                            nodes[i].initiate_connection(peer.local_addr).await?;
                        }
                    }
                }
            }
        }
        Topology::Star => {
            for node in nodes.iter().skip(1) {
                nodes[0].initiate_connection(node.local_addr).await?;
            }
        }
        Topology::None => {}
    }

    Ok(nodes)
}
