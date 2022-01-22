use bytes::Bytes;
use tokio::time::sleep;
use tracing::*;

mod common;
use pea2pea::{
    protocols::{Reading, Writing},
    Pea2Pea,
};

use std::time::Duration;

impl common::MessagingNode {
    fn send_periodic_broadcasts(&self) {
        let self_clone = self.clone();
        tokio::spawn(async move {
            let message = "hello there ( ͡° ͜ʖ ͡°)";
            let bytes = Bytes::from(message.as_bytes());

            loop {
                if self_clone.node().num_connected() != 0 {
                    info!(parent: self_clone.node().span(), "sending \"{}\" to all my frens", message);
                    self_clone.send_broadcast(bytes.clone());
                } else {
                    info!(parent: self_clone.node().span(), "meh, I have no frens to chat with",);
                }

                sleep(Duration::from_millis(50)).await;
            }
        });
    }
}

#[tokio::test]
async fn broadcast_example() {
    // tracing_subscriber::fmt::init();

    let random_nodes = common::start_nodes(4, None)
        .await
        .into_iter()
        .map(common::MessagingNode)
        .collect::<Vec<_>>();
    for rando in &random_nodes {
        rando.enable_reading().await;
    }

    let broadcaster = common::MessagingNode::new("chatty").await;

    broadcaster.enable_writing().await;
    broadcaster.send_periodic_broadcasts();

    for rando in &random_nodes {
        broadcaster
            .0
            .connect(rando.node().listening_addr().unwrap())
            .await
            .unwrap();
    }

    wait_until!(
        1,
        random_nodes
            .iter()
            .all(|rando| rando.node().stats().received().0 == 2)
    );
}
