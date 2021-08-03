use tokio::time::sleep;
use tracing::*;

mod common;
use pea2pea::{
    protocols::{Reading, Writing},
    Node, NodeConfig, Pea2Pea,
};

use std::{io, net::SocketAddr, time::Duration};

#[derive(Clone)]
struct ChattyNode(Node);

impl Pea2Pea for ChattyNode {
    fn node(&self) -> &Node {
        &self.0
    }
}

impl Writing for ChattyNode {
    fn write_message(&self, _: SocketAddr, payload: &[u8], buffer: &mut [u8]) -> io::Result<usize> {
        buffer[..2].copy_from_slice(&(payload.len() as u16).to_le_bytes());
        buffer[2..][..payload.len()].copy_from_slice(payload);
        Ok(2 + payload.len())
    }
}

impl ChattyNode {
    fn send_periodic_broadcasts(&self) {
        let node = self.node().clone();
        tokio::spawn(async move {
            let message = "hello there ( ͡° ͜ʖ ͡°)";
            let bytes = common::prefix_with_len(2, message.as_bytes());

            loop {
                if node.num_connected() != 0 {
                    info!(parent: node.span(), "sending \"{}\" to all my frens", message);
                    if let Err(e) = node.send_broadcast(bytes.clone()) {
                        error!(parent: node.span(), "can't send a broadcast: {}", e);
                    }
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
    let broadcaster = ChattyNode(broadcaster);

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
