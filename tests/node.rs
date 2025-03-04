use deadline::deadline;
use tokio::{
    net::{TcpListener, TcpSocket},
    time::sleep,
};

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

impl Handshake for common::TestNode {
    async fn perform_handshake(
        &self,
        conn: pea2pea::Connection,
    ) -> io::Result<pea2pea::Connection> {
        sleep(Duration::from_millis(50)).await;
        Ok(conn)
    }
}

#[tokio::test]
async fn node_creation_any_port_works() {
    let _node = Node::new(Default::default());
}

#[tokio::test]
async fn node_name_gets_auto_assigned() {
    let node = Node::new(Default::default());
    // ensure that a node without a given name get assigned a numeric ID
    let _: usize = node.config().name.as_ref().unwrap().parse().unwrap();
}

#[tokio::test]
async fn node_given_name_remains_unchanged() {
    let config = Config {
        name: Some("test".into()),
        ..Default::default()
    };
    let node = Node::new(config);
    // ensure that a node with a given name doesn't have it overwritten
    assert_eq!(node.config().name.as_ref().unwrap(), "test");
}

#[tokio::test]
async fn node_use_provided_socket() {
    let connector = Node::new(Default::default());
    let connectee = Node::new(Default::default());
    let connectee_addr = connectee.toggle_listener().await.unwrap().unwrap();

    let socket = TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.77:0".parse().unwrap()).unwrap();

    connector
        .connect_using_socket(connectee_addr, socket)
        .await
        .unwrap();

    let connectee_clone = connectee.clone();
    deadline!(Duration::from_secs(1), move || connectee_clone
        .num_connected()
        == 1);

    assert_eq!(
        connectee.connected_addrs()[0].ip(),
        "127.0.0.77".parse::<Ipv4Addr>().unwrap()
    );
}

#[tokio::test]
async fn node_listener_toggling() {
    let connector = Node::new(Default::default());
    let connectee = Node::new(Default::default());

    for _ in 0..3 {
        assert!(connectee.listening_addr().await.is_err());

        let connectee_addr = connectee.toggle_listener().await.unwrap().unwrap();

        connector.connect(connectee_addr).await.unwrap();

        let connectee_clone = connectee.clone();
        deadline!(Duration::from_secs(1), move || connectee_clone
            .num_connected()
            == 1);

        assert!(connectee.toggle_listener().await.unwrap().is_none());
        assert!(connectee.listening_addr().await.is_err());

        assert!(connector.disconnect(connectee_addr).await);
        assert!(connector.connect(connectee_addr).await.is_err());
    }
}

#[tokio::test]
async fn node_creation_used_port_fails() {
    let config = Config {
        listener_addr: Some("127.0.0.1:9".parse().unwrap()), // the official Discard Protocol port
        ..Default::default()
    };
    let node = Node::new(config);

    assert!(node.toggle_listener().await.is_err());
}

#[tokio::test]
async fn node_connect_and_disconnect() {
    let nodes = Arc::new(common::start_test_nodes(2).await);
    connect_nodes(&nodes, Topology::Line).await.unwrap();
    let node1_addr = nodes[1].node().listening_addr().await.unwrap();

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
    let nodes = Arc::new(common::start_test_nodes(2).await);
    let node1_addr = nodes[1].node().listening_addr().await.unwrap();

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
    let node = Node::new(Default::default());
    let own_addr = node.toggle_listener().await.unwrap().unwrap();
    assert!(node.connect(own_addr).await.is_err());
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
    let connector = Node::new(config);
    let connectee = Node::new(Default::default());
    let connectee_addr = connectee.toggle_listener().await.unwrap().unwrap();

    assert!(connector.connect(connectee_addr).await.is_err());
}

#[tokio::test]
async fn node_connectee_limit_breach_fails() {
    let config = Config {
        max_connections: 0,
        ..Default::default()
    };
    let connectee = Node::new(config);
    let connectee_addr = connectee.toggle_listener().await.unwrap().unwrap();

    let connector = Node::new(Default::default());

    // a breached connection limit doesn't close the listener, so this works
    connector.connect(connectee_addr).await.unwrap();

    // the number of connections on connectee side needs to be checked instead
    deadline!(Duration::from_secs(1), move || connectee.num_connected()
        == 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn node_overlapping_duplicate_connection_attempts_fail() {
    const NUM_ATTEMPTS: usize = 5;

    let connector = Node::new(Default::default());

    let connectee = Node::new(Default::default());
    let connectee_addr = connectee.toggle_listener().await.unwrap().unwrap();

    let err_count = Arc::new(AtomicUsize::new(0));
    for _ in 0..NUM_ATTEMPTS {
        let connector_clone = connector.clone();
        let err_count_clone = err_count.clone();
        tokio::spawn(async move {
            if connector_clone.connect(connectee_addr).await.is_err() {
                err_count_clone.fetch_add(1, Relaxed);
            }
        });
    }

    deadline!(Duration::from_secs(1), move || err_count.load(Relaxed)
        == NUM_ATTEMPTS - 1);
}

#[tokio::test]
async fn node_shutdown_closes_the_listener() {
    let node = Node::new(Default::default());
    let addr = node.toggle_listener().await.unwrap().unwrap();

    assert!(TcpListener::bind(addr).await.is_err());
    node.shut_down().await;
    sleep(Duration::from_millis(100)).await; // the CI needs a delay
    assert!(TcpListener::bind(addr).await.is_ok());
}

#[tokio::test]
async fn test_nodes_use_localhost() {
    let node = Node::new(Default::default());
    let addr = node.toggle_listener().await.unwrap().unwrap();

    assert_eq!(addr.ip(), Ipv4Addr::LOCALHOST);
}
