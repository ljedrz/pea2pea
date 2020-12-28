#![allow(clippy::blocks_in_if_conditions)]

mod common;
use pea2pea::{connect_nodes, Node, Topology};

// the number of nodes spawned for each topology test
const N: usize = 10;

#[tokio::test]
async fn topology_line_conn_counts() {
    let nodes = Node::new_multiple(N, None).await.unwrap();
    connect_nodes(&nodes, Topology::Line).await.unwrap();

    wait_until!(
        1,
        nodes.iter().enumerate().all(|(i, node)| {
            if i == 0 || i == N - 1 {
                node.num_connected() == 1
            } else {
                node.num_connected() == 2
            }
        })
    );
}

#[tokio::test]
async fn topology_ring_conn_counts() {
    let nodes = Node::new_multiple(N, None).await.unwrap();
    connect_nodes(&nodes, Topology::Ring).await.unwrap();

    wait_until!(1, nodes.iter().all(|node| node.num_connected() == 2));
}

#[tokio::test]
async fn topology_mesh_conn_counts() {
    let nodes = Node::new_multiple(N, None).await.unwrap();
    connect_nodes(&nodes, Topology::Mesh).await.unwrap();

    wait_until!(1, nodes.iter().all(|node| node.num_connected() == N - 1));
}

#[tokio::test]
async fn topology_star_conn_counts() {
    let nodes = Node::new_multiple(N, None).await.unwrap();
    connect_nodes(&nodes, Topology::Star).await.unwrap();

    wait_until!(
        1,
        nodes.iter().enumerate().all(|(i, node)| {
            if i == 0 {
                node.num_connected() == N - 1
            } else {
                node.num_connected() == 1
            }
        })
    );
}
