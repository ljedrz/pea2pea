use pea2pea::Node;

#[tokio::test]
async fn node_creation_any_port_works() {
    let _node = Node::new(None, None, None, true).await.unwrap();
}

#[should_panic]
#[tokio::test]
async fn node_creation_bad_params() {
    let _node = Node::new(None, None, None, false).await.unwrap();
}

#[tokio::test]
async fn node_creation_used_port_fails() {
    // TODO: double-check if port 9 (discard protocol) is always unavailable
    assert!(Node::new(None, None, Some(9), false).await.is_err());
}
