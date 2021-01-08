use tokio::net::TcpListener;

mod common;
use pea2pea::{connect_nodes, Node, NodeConfig, Topology};

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[tokio::test]
async fn node_creation_any_port_works() {
    let _node = Node::new(None).await.unwrap();
}

#[should_panic]
#[tokio::test]
async fn node_creation_bad_params() {
    let config = NodeConfig {
        allow_random_port: false,
        ..Default::default()
    };
    let _node = Node::new(Some(config)).await.unwrap();
}

#[tokio::test]
async fn node_creation_used_port_fails() {
    let config = NodeConfig {
        desired_listening_port: Some(9), // the official Discard Protocol port
        allow_random_port: false,
        ..Default::default()
    };
    assert!(Node::new(Some(config)).await.is_err());
}

#[tokio::test]
async fn node_connect_and_disconnect() {
    let nodes = common::start_inert_nodes(2, None).await;
    connect_nodes(&nodes, Topology::Line).await.unwrap();

    assert!(nodes[0].disconnect(nodes[1].listening_addr()));
    assert!(!nodes[0].is_connected(nodes[1].listening_addr()));
}

#[tokio::test]
async fn node_self_connection_fails() {
    let node = Node::new(None).await.unwrap();
    assert!(node.connect(node.listening_addr()).await.is_err());
}

#[tokio::test]
async fn node_duplicate_connection_fails() {
    let nodes = common::start_inert_nodes(2, None).await;
    assert!(connect_nodes(&nodes, Topology::Line).await.is_ok());
    assert!(connect_nodes(&nodes, Topology::Line).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn node_overlapping_duplicate_connection_attempts_fail() {
    const NUM_ATTEMPTS: usize = 5;

    let connector = Node::new(None).await.unwrap();
    let connectee = Node::new(None).await.unwrap();
    let addr = connectee.listening_addr();

    let err_count = Arc::new(AtomicUsize::new(0));
    for _ in 0..NUM_ATTEMPTS {
        let connector_clone = connector.clone();
        let err_count_clone = err_count.clone();
        tokio::spawn(async move {
            if connector_clone.connect(addr).await.is_err() {
                err_count_clone.fetch_add(1, Ordering::Relaxed);
            }
        });
    }

    wait_until!(1, err_count.load(Ordering::Relaxed) == NUM_ATTEMPTS - 1);
}

#[tokio::test]
async fn drop_shuts_the_listener() {
    let node = Node::new(None).await.unwrap();
    let addr = node.listening_addr();

    assert!(TcpListener::bind(addr).await.is_err());
    node.shut_down();
    assert!(TcpListener::bind(addr).await.is_ok());
}
