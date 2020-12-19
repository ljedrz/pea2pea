use pea2pea::{spawn_nodes, Topology};
use tokio::time::sleep;

use std::time::Duration;

#[tokio::test]
async fn start_and_cancel_connecting() {
    let nodes = spawn_nodes(2, Topology::Line).await.unwrap();
    sleep(Duration::from_millis(200)).await;

    assert!(nodes[0].disconnect(nodes[1].local_addr()));
}
