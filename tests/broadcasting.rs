use tokio::time::sleep;
use tracing::*;

mod common;
use pea2pea::{ContainsNode, Messaging, Node, NodeConfig};

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
            let bytes = common::prefix_message_with_len(2, message.as_bytes());

            loop {
                if node.num_handshaken() != 0 {
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
async fn broadcast_example() {
    tracing_subscriber::fmt::init();

    let random_nodes = Node::new_multiple(4, None)
        .await
        .unwrap()
        .into_iter()
        .map(common::RandomNode)
        .collect::<Vec<_>>();
    for rando in &random_nodes {
        rando.enable_messaging();
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

    wait_until!(
        1,
        random_nodes
            .iter()
            .all(|rando| rando.node().num_messages_received() > 0)
    );
}
