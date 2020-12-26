use bytes::Bytes;
use tokio::time::sleep;
use tracing::*;

mod common;
use pea2pea::{ContainsNode, MessagingProtocol, Node, NodeConfig};

use std::{sync::Arc, time::Duration};

#[derive(Clone)]
struct ChattyNode(Arc<Node>);

impl ContainsNode for ChattyNode {
    fn node(&self) -> &Arc<Node> {
        &self.0
    }
}

impl ChattyNode {
    fn send_periodic_broadcasts(&self) {
        let node = Arc::clone(&self.node());
        tokio::spawn(async move {
            let message = "hello there ( ͡° ͜ʖ ͡°)";
            let u16_len = (message.len() as u16).to_le_bytes();
            let mut bytes = Vec::with_capacity(2 + message.len());
            bytes.extend_from_slice(&u16_len);
            bytes.extend_from_slice(message.as_bytes());
            let bytes = Bytes::from(bytes);

            loop {
                if !node.handshaken_addrs().is_empty() {
                    info!(parent: node.span(), "sending \"{}\" to all my frens", message);
                    node.send_broadcast(bytes.clone()).await;
                } else {
                    info!(parent: node.span(), "meh, I have no frens to chat with",);
                }

                sleep(Duration::from_millis(50)).await;
            }
        });
    }
}

#[tokio::test]
async fn broadcast_protocol() {
    tracing_subscriber::fmt::init();

    let random_nodes = Node::new_multiple(4, None)
        .await
        .unwrap()
        .into_iter()
        .map(common::RandomNode)
        .collect::<Vec<_>>();
    for rando in &random_nodes {
        rando.enable_messaging_protocol();
    }

    let mut broadcaster_config = NodeConfig::default();
    broadcaster_config.name = Some("chatty".into());
    let broadcaster = Node::new(Some(broadcaster_config)).await.unwrap();
    let broadcaster = ChattyNode(broadcaster);

    broadcaster.send_periodic_broadcasts();

    for rando in &random_nodes {
        broadcaster
            .0
            .initiate_connection(rando.node().listening_addr)
            .await
            .unwrap();
    }

    sleep(Duration::from_millis(100)).await;

    for rando in &random_nodes {
        assert!(rando.node().num_messages_received() != 0);
    }
}
