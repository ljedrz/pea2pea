use pea2pea::{spawn_nodes, Topology};
use tokio::time::sleep;

use std::time::Duration;

// the number of nodes spawned for each topology test
const N: usize = 5;

#[tokio::test]
async fn topology_line_conn_counts() {
    let nodes = spawn_nodes(N, Topology::Line).await.unwrap();
    sleep(Duration::from_millis(200)).await;

    for (i, node) in nodes.iter().enumerate() {
        if i == 0 || i == N - 1 {
            assert_eq!(node.num_connecting(), 1);
        } else {
            assert_eq!(node.num_connecting(), 2);
        }
    }
}

#[tokio::test]
async fn topology_ring_conn_counts() {
    let nodes = spawn_nodes(N, Topology::Ring).await.unwrap();
    sleep(Duration::from_millis(200)).await;

    for node in &nodes {
        assert_eq!(node.num_connecting(), 2);
    }
}

#[tokio::test]
async fn topology_mesh_conn_counts() {
    let nodes = spawn_nodes(N, Topology::Mesh).await.unwrap();
    sleep(Duration::from_millis(200)).await;

    for node in &nodes {
        assert_eq!(node.num_connecting(), N - 1);
    }
}

#[tokio::test]
async fn topology_star_conn_counts() {
    let nodes = spawn_nodes(N, Topology::Star).await.unwrap();
    sleep(Duration::from_millis(200)).await;

    for (i, node) in nodes.iter().enumerate() {
        if i == 0 {
            assert_eq!(node.num_connecting(), N - 1);
        } else {
            assert_eq!(node.num_connecting(), 1);
        }
    }
}
