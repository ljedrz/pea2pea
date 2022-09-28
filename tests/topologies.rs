#![allow(clippy::blocks_in_if_conditions)]

mod common;
use std::time::Duration;

use deadline::deadline;
use pea2pea::{connect_nodes, Pea2Pea, Topology};

// the number of nodes spawned for each topology test
const N: usize = 10;

#[tokio::test]
async fn topology_line_conn_counts() {
    let nodes = common::start_test_nodes(N).await;
    connect_nodes(&nodes, Topology::Line).await.unwrap();

    deadline!(Duration::from_secs(1), move || nodes
        .iter()
        .enumerate()
        .all(|(i, node)| {
            if i == 0 || i == N - 1 {
                node.node().num_connected() == 1
            } else {
                node.node().num_connected() == 2
            }
        }));
}

#[tokio::test]
async fn topology_ring_conn_counts() {
    let nodes = common::start_test_nodes(N).await;
    connect_nodes(&nodes, Topology::Ring).await.unwrap();

    deadline!(Duration::from_secs(1), move || nodes
        .iter()
        .all(|node| node.node().num_connected() == 2));
}

#[tokio::test]
async fn topology_mesh_conn_counts() {
    let nodes = common::start_test_nodes(N).await;
    connect_nodes(&nodes, Topology::Mesh).await.unwrap();

    deadline!(Duration::from_secs(1), move || nodes
        .iter()
        .all(|n| n.node().num_connected() == N - 1));
}

#[tokio::test]
async fn topology_star_conn_counts() {
    let nodes = common::start_test_nodes(N).await;
    connect_nodes(&nodes, Topology::Star).await.unwrap();

    deadline!(Duration::from_secs(1), move || nodes
        .iter()
        .enumerate()
        .all(|(i, node)| {
            if i == 0 {
                node.node().num_connected() == N - 1
            } else {
                node.node().num_connected() == 1
            }
        }));
}
