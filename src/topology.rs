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
                    .initiate_connection(nodes[i + 1].listening_addr)
                    .await?;
            }
            if topology == Topology::Ring {
                nodes[count - 1]
                    .initiate_connection(nodes[0].listening_addr)
                    .await?;
            }
        }
        Topology::Mesh => {
            let mut connected_pairs = HashSet::with_capacity((count - 1) * 2);
            for i in 0..count {
                for (j, peer) in nodes.iter().enumerate() {
                    if i != j && connected_pairs.insert((i, j)) && connected_pairs.insert((j, i)) {
                        nodes[i].initiate_connection(peer.listening_addr).await?;
                    }
                }
            }
        }
        Topology::Star => {
            for node in nodes.iter().skip(1) {
                nodes[0].initiate_connection(node.listening_addr).await?;
            }
        }
        Topology::None => {}
    }

    Ok(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::time::sleep;

    use std::time::Duration;

    // the number of nodes spawned for each topology test
    const N: usize = 10;

    #[tokio::test]
    async fn topology_line_conn_counts() {
        let nodes = spawn_nodes(N, Topology::Line).await.unwrap();
        sleep(Duration::from_millis(200)).await;

        for (i, node) in nodes.iter().enumerate() {
            if i == 0 || i == N - 1 {
                assert_eq!(node.num_connected(), 1);
            } else {
                assert_eq!(node.num_connected(), 2);
            }
        }
    }

    #[tokio::test]
    async fn topology_ring_conn_counts() {
        let nodes = spawn_nodes(N, Topology::Ring).await.unwrap();
        sleep(Duration::from_millis(200)).await;

        for node in &nodes {
            assert_eq!(node.num_connected(), 2);
        }
    }

    #[tokio::test]
    async fn topology_mesh_conn_counts() {
        let nodes = spawn_nodes(N, Topology::Mesh).await.unwrap();
        sleep(Duration::from_millis(200)).await;

        for node in &nodes {
            assert_eq!(node.num_connected(), N - 1);
        }
    }

    #[tokio::test]
    async fn topology_star_conn_counts() {
        let nodes = spawn_nodes(N, Topology::Star).await.unwrap();
        sleep(Duration::from_millis(200)).await;

        for (i, node) in nodes.iter().enumerate() {
            if i == 0 {
                assert_eq!(node.num_connected(), N - 1);
            } else {
                assert_eq!(node.num_connected(), 1);
            }
        }
    }
}
