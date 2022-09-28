use bytes::Bytes;
use deadline::deadline;
use tokio::time::sleep;
use tracing::*;

mod common;
use std::time::Duration;

use pea2pea::{
    protocols::{Reading, Writing},
    Pea2Pea,
};

impl common::TestNode {
    fn send_periodic_broadcasts(&self) {
        let self_clone = self.clone();
        tokio::spawn(async move {
            let message = "hello there ( ͡° ͜ʖ ͡°)";
            let bytes = Bytes::from(message.as_bytes());

            loop {
                if self_clone.node().num_connected() != 0 {
                    info!(parent: self_clone.node().span(), "sending \"{}\" to all my frens", message);
                    self_clone.broadcast(bytes.clone()).unwrap();
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
    let random_nodes = common::start_test_nodes(4).await;
    for rando in &random_nodes {
        rando.enable_reading().await;
    }

    let broadcaster = crate::test_node!("chatty");

    broadcaster.enable_writing().await;
    broadcaster.send_periodic_broadcasts();

    for rando in &random_nodes {
        broadcaster
            .0
            .connect(rando.node().listening_addr().unwrap())
            .await
            .unwrap();
    }

    deadline!(Duration::from_secs(1), move || random_nodes
        .iter()
        .all(|rando| rando.node().stats().received().0 >= 2));
}
