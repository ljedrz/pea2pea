use tokio::time::sleep;
use tracing::*;

mod common;
use pea2pea::{
    protocols::{Reading, Writing},
    Node, NodeConfig, Pea2Pea,
};

use std::time::Duration;

impl common::MessagingNode {
    fn send_periodic_broadcasts(&self) {
        let node = self.node().clone();
        tokio::spawn(async move {
            let message = "hello there ( ͡° ͜ʖ ͡°)";
            let bytes = common::prefix_with_len(4, message.as_bytes());

            loop {
                if node.num_connected() != 0 {
                    info!(parent: node.span(), "sending \"{}\" to all my frens", message);
                    node.send_broadcast(bytes.clone());
                } else {
                    info!(parent: node.span(), "meh, I have no frens to chat with",);
                }

                sleep(Duration::from_millis(50)).await;
            }
        });
    }
}

#[tokio::test]
async fn broadcast_example() {
    tracing_subscriber::fmt::init();

    let random_nodes = common::start_nodes(4, None)
        .await
        .into_iter()
        .map(common::MessagingNode)
        .collect::<Vec<_>>();
    for rando in &random_nodes {
        rando.enable_reading();
    }

    let broadcaster_config = NodeConfig {
        name: Some("chatty".into()),
        listener_ip: "127.0.0.1".parse().unwrap(),
        ..Default::default()
    };
    let broadcaster = Node::new(Some(broadcaster_config)).await.unwrap();
    let broadcaster = common::MessagingNode(broadcaster);

    broadcaster.enable_writing();
    broadcaster.send_periodic_broadcasts();

    for rando in &random_nodes {
        broadcaster
            .0
            .connect(rando.node().listening_addr())
            .await
            .unwrap();
    }

    wait_until!(
        1,
        random_nodes
            .iter()
            .all(|rando| rando.node().stats().received().0 != 0)
    );
}
