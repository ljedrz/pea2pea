use pea2pea::{Node, NodeConfig};

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
