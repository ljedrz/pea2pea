use pea2pea::{spawn_nodes, Node, NodeConfig, Topology};
use tokio::time::sleep;

use std::time::Duration;

#[tokio::test]
async fn node_creation_any_port_works() {
    let _node = Node::new(None).await.unwrap();
}

#[should_panic]
#[tokio::test]
async fn node_creation_bad_params() {
    let mut config = NodeConfig::default();
    config.allow_random_port = false;
    let _node = Node::new(Some(config)).await.unwrap();
}

#[tokio::test]
async fn node_creation_used_port_fails() {
    // TODO: double-check if port 9 (discard protocol) is always unavailable
    let mut config = NodeConfig::default();
    config.desired_listening_port = Some(9);
    config.allow_random_port = false;
    assert!(Node::new(Some(config)).await.is_err());
}

#[tokio::test]
async fn start_and_cancel_connecting() {
    let nodes = spawn_nodes(2, Topology::Line, None).await.unwrap();
    sleep(Duration::from_millis(200)).await;

    assert!(nodes[0].disconnect(nodes[1].listening_addr));
    assert!(!nodes[0].is_connected(nodes[1].listening_addr));
}
