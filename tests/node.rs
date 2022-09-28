use deadline::deadline;
use tokio::{net::TcpListener, time::sleep};

mod common;
use std::{
    io,
    net::Ipv4Addr,
    sync::{
        atomic::{AtomicUsize, Ordering::Relaxed},
        Arc,
    },
    time::Duration,
};

use pea2pea::{connect_nodes, protocols::Handshake, Config, Node, Pea2Pea, Topology};

#[tokio::test]
async fn node_creation_any_port_works() {
    let _node = Node::new(Default::default()).await.unwrap();
}

#[tokio::test]
async fn node_name_gets_auto_assigned() {
    let node = Node::new(Default::default()).await.unwrap();
    // ensure that a node without a given name get assigned a numeric ID
    let _: usize = node.config().name.as_ref().unwrap().parse().unwrap();
}

#[tokio::test]
async fn node_given_name_remains_unchanged() {
    let config = Config {
        name: Some("test".into()),
        ..Default::default()
    };
    let node = Node::new(config).await.unwrap();
    // ensure that a node with a given name doesn't have it overwritten
    assert_eq!(node.config().name.as_ref().unwrap(), "test");
}

#[should_panic]
#[tokio::test]
async fn node_creation_bad_params_panic() {
    let config = Config {
        allow_random_port: false,
        ..Default::default()
    };
    let _node = Node::new(config).await.unwrap();
}

#[tokio::test]
async fn node_creation_used_port_fails() {
    let config = Config {
        desired_listening_port: Some(9), // the official Discard Protocol port
        allow_random_port: false,
        ..Default::default()
    };
    assert!(Node::new(config).await.is_err());
}

#[tokio::test]
async fn node_connect_and_disconnect() {
    let nodes = Arc::new(common::start_test_nodes(2).await);
    connect_nodes(&nodes, Topology::Line).await.unwrap();
    let node1_addr = nodes[1].node().listening_addr().unwrap();

    let nodes_clone = nodes.clone();
    deadline!(Duration::from_secs(1), move || nodes_clone
        .iter()
        .all(|n| n.node().num_connected() == 1));

    assert!(nodes.iter().all(|n| n.node().num_connecting() == 0));

    assert!(nodes[0].node().is_connected(node1_addr));

    assert!(nodes[0].node().disconnect(node1_addr).await);

    assert!(!nodes[0].node().is_connected(node1_addr));

    // node[1] didn't enable reading, so it has no way of knowing
    // that the connection has been broken by node[0]
    assert_eq!(nodes[0].node().num_connected(), 0);
    assert_eq!(nodes[1].node().num_connected(), 1);
}

#[tokio::test]
async fn node_connecting() {
    #[async_trait::async_trait]
    impl Handshake for common::TestNode {
        async fn perform_handshake(
            &self,
            conn: pea2pea::Connection,
        ) -> io::Result<pea2pea::Connection> {
            sleep(Duration::from_millis(50)).await;
            Ok(conn)
        }
    }

    let nodes = Arc::new(common::start_test_nodes(2).await);
    let node1_addr = nodes[1].node().listening_addr().unwrap();

    assert!(!nodes[0].node().is_connecting(node1_addr));

    let node0 = nodes[0].clone();
    tokio::spawn(async move {
        node0.node().connect(node1_addr).await.unwrap();
    });

    let node0 = nodes[0].clone();
    deadline!(Duration::from_millis(100), move || node0
        .node()
        .is_connecting(node1_addr));

    let nodes_clone = nodes.clone();
    deadline!(Duration::from_secs(1), move || nodes_clone
        .iter()
        .all(|n| n.node().num_connected() == 1));
}

#[tokio::test]
async fn node_self_connection_fails() {
    let node = Node::new(Default::default()).await.unwrap();
    assert!(node.connect(node.listening_addr().unwrap()).await.is_err());
}

#[tokio::test]
async fn node_duplicate_connection_fails() {
    let nodes = common::start_test_nodes(2).await;
    assert!(connect_nodes(&nodes, Topology::Line).await.is_ok());
    assert!(connect_nodes(&nodes, Topology::Line).await.is_err());
}

#[tokio::test]
async fn node_two_way_connection_works() {
    let mut nodes = common::start_test_nodes(2).await;
    assert!(connect_nodes(&nodes, Topology::Line).await.is_ok());
    nodes.reverse();
    assert!(connect_nodes(&nodes, Topology::Line).await.is_ok());
}

#[tokio::test]
async fn node_connector_limit_breach_fails() {
    let config = Config {
        max_connections: 0,
        ..Default::default()
    };
    let connector = Node::new(config).await.unwrap();
    let connectee = Node::new(Default::default()).await.unwrap();

    assert!(connector
        .connect(connectee.listening_addr().unwrap())
        .await
        .is_err());
}

#[tokio::test]
async fn node_connectee_limit_breach_fails() {
    let config = Config {
        max_connections: 0,
        ..Default::default()
    };
    let connectee = Node::new(config).await.unwrap();
    let connector = Node::new(Default::default()).await.unwrap();

    // a breached connection limit doesn't close the listener, so this works
    connector
        .connect(connectee.listening_addr().unwrap())
        .await
        .unwrap();

    // the number of connections on connectee side needs to be checked instead
    deadline!(Duration::from_secs(1), move || connectee.num_connected()
        == 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn node_overlapping_duplicate_connection_attempts_fail() {
    const NUM_ATTEMPTS: usize = 5;

    let connector = Node::new(Default::default()).await.unwrap();
    let connectee = Node::new(Default::default()).await.unwrap();
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

    deadline!(Duration::from_secs(1), move || err_count.load(Relaxed)
        == NUM_ATTEMPTS - 1);
}

#[tokio::test]
async fn node_shutdown_closes_the_listener() {
    let node = Node::new(Default::default()).await.unwrap();
    let addr = node.listening_addr().unwrap();

    assert!(TcpListener::bind(addr).await.is_err());
    node.shut_down().await;
    sleep(Duration::from_millis(100)).await; // the CI needs a delay
    assert!(TcpListener::bind(addr).await.is_ok());
}

#[tokio::test]
async fn test_nodes_use_localhost() {
    let node = Node::new(Default::default()).await.unwrap();

    assert_eq!(node.listening_addr().unwrap().ip(), Ipv4Addr::LOCALHOST);
}
