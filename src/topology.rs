use crate::{ContainsNode, Node, NodeConfig};

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

pub async fn spawn_nodes(count: usize, config: Option<NodeConfig>) -> io::Result<Vec<Arc<Node>>> {
    let mut nodes = Vec::with_capacity(count);

    for _ in 0..count {
        let node = Node::new(config.clone()).await?;
        nodes.push(node);
    }

    Ok(nodes)
}

pub async fn connect_nodes<T: ContainsNode>(nodes: &[T], topology: Topology) -> io::Result<()> {
    let count = nodes.len();

    match topology {
        Topology::Line | Topology::Ring => {
            for i in 0..(count - 1) {
                nodes[i]
                    .node()
                    .initiate_connection(nodes[i + 1].node().listening_addr)
                    .await?;
            }
            if topology == Topology::Ring {
                nodes[count - 1]
                    .node()
                    .initiate_connection(nodes[0].node().listening_addr)
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
                            .initiate_connection(peer.node().listening_addr)
                            .await?;
                    }
                }
            }
        }
        Topology::Star => {
            for node in nodes.iter().skip(1) {
                nodes[0]
                    .node()
                    .initiate_connection(node.node().listening_addr)
                    .await?;
            }
        }
        Topology::None => {}
    }

    Ok(())
}
