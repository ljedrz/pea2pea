use tokio::net::TcpListener;

mod common;
use pea2pea::{connect_nodes, Node, NodeConfig, Topology};

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
async fn node_duplicate_connection() {
    let nodes = common::start_inert_nodes(2, None).await;
    assert!(connect_nodes(&nodes, Topology::Line).await.is_ok());
    assert!(connect_nodes(&nodes, Topology::Line).await.is_err());
}

#[tokio::test]
async fn drop_shuts_the_listener() {
    let node = Node::new(None).await.unwrap();
    let addr = node.listening_addr();

    assert!(TcpListener::bind(addr).await.is_err());
    node.shut_down();
    assert!(TcpListener::bind(addr).await.is_ok());
}
