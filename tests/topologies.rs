use tokio::time::sleep;

use pea2pea::{connect_nodes, spawn_nodes, Topology};

use std::time::Duration;

// the number of nodes spawned for each topology test
const N: usize = 10;

#[tokio::test]
async fn topology_line_conn_counts() {
    let nodes = spawn_nodes(N, None).await.unwrap();
    connect_nodes(&nodes, Topology::Line).await.unwrap();
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
    let nodes = spawn_nodes(N, None).await.unwrap();
    connect_nodes(&nodes, Topology::Ring).await.unwrap();
    sleep(Duration::from_millis(200)).await;

    for node in &nodes {
        assert_eq!(node.num_connected(), 2);
    }
}

#[tokio::test]
async fn topology_mesh_conn_counts() {
    let nodes = spawn_nodes(N, None).await.unwrap();
    connect_nodes(&nodes, Topology::Mesh).await.unwrap();
    sleep(Duration::from_millis(200)).await;

    for node in &nodes {
        assert_eq!(node.num_connected(), N - 1);
    }
}

#[tokio::test]
async fn topology_star_conn_counts() {
    let nodes = spawn_nodes(N, None).await.unwrap();
    connect_nodes(&nodes, Topology::Star).await.unwrap();
    sleep(Duration::from_millis(200)).await;

    for (i, node) in nodes.iter().enumerate() {
        if i == 0 {
            assert_eq!(node.num_connected(), N - 1);
        } else {
            assert_eq!(node.num_connected(), 1);
        }
    }
}
