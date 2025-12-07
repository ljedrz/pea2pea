mod common;
use std::{net::SocketAddr, sync::Arc};

use pea2pea::{Pea2Pea, Topology, connect_nodes, protocols::OnConnect};
use tokio::sync::Barrier;

// the number of nodes spawned for each topology test
const N: usize = 10;

crate::impl_barrier_on_connect!(common::TestNode);

#[tokio::test]
async fn topology_line_conn_counts() {
    let nodes = common::start_test_nodes(N).await;
    let barrier = Arc::new(Barrier::new(
        (Topology::Line).num_expected_connections(N) + 1,
    ));
    for node in &nodes {
        node.barrier.set(barrier.clone()).unwrap();
        node.enable_on_connect().await;
    }
    connect_nodes(&nodes, Topology::Line).await.unwrap();
    barrier.wait().await;

    assert!(nodes.iter().enumerate().all(|(i, node)| {
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
    let barrier = Arc::new(Barrier::new(
        (Topology::Ring).num_expected_connections(N) + 1,
    ));
    for node in &nodes {
        node.barrier.set(barrier.clone()).unwrap();
        node.enable_on_connect().await;
    }
    connect_nodes(&nodes, Topology::Ring).await.unwrap();
    barrier.wait().await;

    assert!(nodes.iter().all(|node| node.node().num_connected() == 2));
}

#[tokio::test]
async fn topology_mesh_conn_counts() {
    let nodes = common::start_test_nodes(N).await;
    let barrier = Arc::new(Barrier::new(
        (Topology::Mesh).num_expected_connections(N) + 1,
    ));
    for node in &nodes {
        node.barrier.set(barrier.clone()).unwrap();
        node.enable_on_connect().await;
    }
    connect_nodes(&nodes, Topology::Mesh).await.unwrap();
    barrier.wait().await;

    assert!(nodes.iter().all(|n| n.node().num_connected() == N - 1));
}

#[tokio::test]
async fn topology_star_conn_counts() {
    let nodes = common::start_test_nodes(N).await;
    let barrier = Arc::new(Barrier::new(
        (Topology::Star).num_expected_connections(N) + 1,
    ));
    for node in &nodes {
        node.barrier.set(barrier.clone()).unwrap();
        node.enable_on_connect().await;
    }
    connect_nodes(&nodes, Topology::Star).await.unwrap();
    barrier.wait().await;

    assert!(nodes.iter().enumerate().all(|(i, node)| {
        if i == 0 {
            node.node().num_connected() == N - 1
        } else {
            node.node().num_connected() == 1
        }
    }));
}

#[tokio::test]
async fn topology_grid_conn_counts() {
    let nodes = common::start_test_nodes(N).await;
    let topology = Topology::Grid {
        width: 5,
        height: 2,
    };
    let barrier = Arc::new(Barrier::new(topology.num_expected_connections(N) + 1));
    for node in &nodes {
        node.barrier.set(barrier.clone()).unwrap();
        node.enable_on_connect().await;
    }
    connect_nodes(&nodes, topology).await.unwrap();
    barrier.wait().await;

    assert!(nodes.iter().enumerate().all(|(i, node)| {
        let row = i / 5;
        let col = i % 5;
        let mut expected = 0;

        // check Up
        if row > 0 {
            expected += 1;
        }
        // check Down (height is 2, so max row is 1)
        if row < 1 {
            expected += 1;
        }
        // check Left
        if col > 0 {
            expected += 1;
        }
        // check Right (width is 5, so max col is 4)
        if col < 4 {
            expected += 1;
        }

        node.node().num_connected() == expected
    }));
}

#[tokio::test]
async fn topology_tree_conn_counts() {
    let nodes = common::start_test_nodes(N).await;
    let barrier = Arc::new(Barrier::new(
        (Topology::Tree).num_expected_connections(N) + 1,
    ));
    for node in &nodes {
        node.barrier.set(barrier.clone()).unwrap();
        node.enable_on_connect().await;
    }
    connect_nodes(&nodes, Topology::Tree).await.unwrap();
    barrier.wait().await;

    assert!(nodes.iter().enumerate().all(|(i, node)| {
        let mut expected = 0;

        // check for parent (Node 0 has no parent)
        if i > 0 {
            expected += 1;
        }

        // check for left child
        if 2 * i + 1 < N {
            expected += 1;
        }

        // check for right child
        if 2 * i + 2 < N {
            expected += 1;
        }

        node.node().num_connected() == expected
    }));
}

#[tokio::test]
async fn topology_random_conn_counts() {
    let nodes = common::start_test_nodes(N).await;
    let topology = Topology::Random {
        degree: 2,
        seed: 12345,
    };
    let barrier = Arc::new(Barrier::new(topology.num_expected_connections(N) + 1));
    for node in &nodes {
        node.barrier.set(barrier.clone()).unwrap();
        node.enable_on_connect().await;
    }
    connect_nodes(&nodes, topology).await.unwrap();
    barrier.wait().await;

    // verify every node has at least the minimum degree (2)
    assert!(nodes.iter().all(|node| node.node().num_connected() >= 2));

    // verify the global total of connections matches the expectation
    let total_connections: usize = nodes.iter().map(|n| n.node().num_connected()).sum();

    // N (10) * degree (2) * 2 sides = 40
    assert_eq!(total_connections, N * 2 * 2);
}
