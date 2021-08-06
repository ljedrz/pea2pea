use tokio::{net::TcpListener, time::sleep};

mod common;
use pea2pea::{connect_nodes, Node, NodeConfig, Topology};

use std::{
    net::Ipv4Addr,
    sync::{
        atomic::{AtomicUsize, Ordering::Relaxed},
        Arc,
    },
    time::Duration,
};

#[tokio::test]
async fn node_creation_any_port_works() {
    let _node = Node::new(None).await.unwrap();
}

#[should_panic]
#[tokio::test]
async fn node_creation_bad_params_panic() {
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

    wait_until!(1, nodes.iter().all(|node| node.num_connected() == 1));

    assert!(nodes.iter().all(|node| node.num_connecting() == 0));

    assert!(
        nodes[0]
            .disconnect(nodes[1].listening_addr().unwrap())
            .await
    );

    wait_until!(1, nodes[0].num_connected() == 0);

    // node[1] didn't enable reading, so it has no way of knowing
    // that the connection has been broken by node[0]
    assert_eq!(nodes[1].num_connected(), 1);
}

#[tokio::test]
async fn node_self_connection_fails() {
    let node = Node::new(None).await.unwrap();
    assert!(node.connect(node.listening_addr().unwrap()).await.is_err());
}

#[tokio::test]
async fn node_duplicate_connection_fails() {
    let nodes = common::start_inert_nodes(2, None).await;
    assert!(connect_nodes(&nodes, Topology::Line).await.is_ok());
    assert!(connect_nodes(&nodes, Topology::Line).await.is_err());
}

#[tokio::test]
async fn node_two_way_connection_works() {
    let mut nodes = common::start_inert_nodes(2, None).await;
    assert!(connect_nodes(&nodes, Topology::Line).await.is_ok());
    nodes.reverse();
    assert!(connect_nodes(&nodes, Topology::Line).await.is_ok());
}

#[tokio::test]
async fn node_connector_limit_breach_fails() {
    let config = NodeConfig {
        max_connections: 0,
        ..Default::default()
    };
    let connector = Node::new(Some(config)).await.unwrap();
    let connectee = Node::new(None).await.unwrap();

    assert!(connector
        .connect(connectee.listening_addr().unwrap())
        .await
        .is_err());
}

#[tokio::test]
async fn node_connectee_limit_breach_fails() {
    let config = NodeConfig {
        max_connections: 0,
        ..Default::default()
    };
    let connectee = Node::new(Some(config)).await.unwrap();
    let connector = Node::new(None).await.unwrap();

    // a breached connection limit doesn't close the listener, so this works
    connector
        .connect(connectee.listening_addr().unwrap())
        .await
        .unwrap();

    // the number of connections on connectee side needs to be checked instead
    wait_until!(1, connectee.num_connected() == 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn node_overlapping_duplicate_connection_attempts_fail() {
    const NUM_ATTEMPTS: usize = 5;

    let connector = Node::new(None).await.unwrap();
    let connectee = Node::new(None).await.unwrap();
    let addr = connectee.listening_addr().unwrap();

    let err_count = Arc::new(AtomicUsize::new(0));
    for _ in 0..NUM_ATTEMPTS {
        let connector_clone = connector.clone();
        let err_count_clone = err_count.clone();
        tokio::spawn(async move {
            if connector_clone.connect(addr).await.is_err() {
                err_count_clone.fetch_add(1, Relaxed);
            }
        });
    }

    wait_until!(1, err_count.load(Relaxed) == NUM_ATTEMPTS - 1);
}

#[tokio::test]
async fn node_shutdown_closes_the_listener() {
    let node = Node::new(None).await.unwrap();
    let addr = node.listening_addr().unwrap();

    assert!(TcpListener::bind(addr).await.is_err());
    node.shut_down().await;
    sleep(Duration::from_millis(100)).await; // the CI needs a delay
    assert!(TcpListener::bind(addr).await.is_ok());
}

#[tokio::test]
async fn test_nodes_use_localhost() {
    let node = Node::new(None).await.unwrap();

    assert_eq!(node.listening_addr().unwrap().ip(), Ipv4Addr::LOCALHOST);
}
