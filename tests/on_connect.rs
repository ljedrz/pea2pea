use deadline::deadline;
use tokio::time::sleep;

mod common;
use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

use pea2pea::{
    Node, Pea2Pea,
    protocols::{OnConnect, Reading, Writing},
};

use crate::common::WritingExt;

impl OnConnect for common::TestNode {
    async fn on_connect(&self, peer_addr: SocketAddr) {
        self.send_dm(peer_addr, "connected".into()).await.unwrap();
    }
}

#[tokio::test]
async fn on_connect_message() {
    let connector = crate::test_node!("connector");
    connector.enable_writing().await;
    connector.enable_on_connect().await;

    let connectee = crate::test_node!("connectee");
    connectee.enable_reading().await;
    let connectee_addr = connectee.node().toggle_listener().await.unwrap().unwrap();

    connector.node().connect(connectee_addr).await.unwrap();

    let connectee_clone = connectee.clone();
    deadline!(Duration::from_secs(1), move || connectee_clone
        .node()
        .num_connected()
        == 1);

    deadline!(Duration::from_secs(1), move || {
        connectee.node().stats().received().0 == 1
    });
}

#[tokio::test]
async fn connect_doesnt_wait_for_on_connect_hook() {
    #[derive(Clone)]
    struct SlowNode(Node);

    impl Pea2Pea for SlowNode {
        fn node(&self) -> &Node {
            &self.0
        }
    }

    impl OnConnect for SlowNode {
        async fn on_connect(&self, _addr: SocketAddr) {
            // simulate a slow initialization step (e.g., DB lookup, auth)
            sleep(Duration::from_millis(500)).await;
        }
    }

    let initiator = SlowNode(Node::new(Default::default()));
    initiator.enable_on_connect().await;

    let target = crate::test_node!("target");
    let target_addr = target.node().toggle_listener().await.unwrap().unwrap();

    let start = Instant::now();

    // this should not return until the 500ms sleep in on_connect finishes
    initiator.node().connect(target_addr).await.unwrap();

    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_millis(500),
        "node.connect() returned too late!"
    );
    assert_eq!(initiator.node().num_connected(), 1);
}
