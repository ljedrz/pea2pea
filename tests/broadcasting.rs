use tokio::time::sleep;
use tracing::*;

use pea2pea::{BroadcastProtocol, Node, NodeConfig};

use std::{ops::Deref, sync::Arc, time::Duration};

#[derive(Clone)]
struct ChattyNode(Arc<Node>);

impl Deref for ChattyNode {
    type Target = Node;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl BroadcastProtocol for ChattyNode {
    const INTERVAL_MS: u64 = 100;

    fn enable_broadcast_protocol(self: &Arc<Self>) {
        let node = Arc::clone(self);
        tokio::spawn(async move {
            let message = "hello there ( ͡° ͜ʖ ͡°)";
            loop {
                info!(parent: node.span(), "sending \"{}\" to all my frens", message);
                node.send_broadcast(message.as_bytes().to_vec()).await;
                sleep(Duration::from_millis(Self::INTERVAL_MS)).await;
            }
        });
    }
}

#[tokio::test]
async fn chatty_node_broadcasts() {
    tracing_subscriber::fmt::init();

    let mut generic_node_config = NodeConfig::default();
    generic_node_config.name = Some("generic".into());
    let generic_node = Node::new(Some(generic_node_config)).await.unwrap();

    let mut chatty_node_config = NodeConfig::default();
    chatty_node_config.name = Some("chatty".into());
    let chatty_node = Node::new(Some(chatty_node_config)).await.unwrap();
    let chatty_node = Arc::new(ChattyNode(chatty_node));

    chatty_node
        .0
        .initiate_connection(generic_node.local_addr)
        .await
        .unwrap();
    chatty_node.enable_broadcast_protocol();

    sleep(Duration::from_millis(100)).await;

    assert!(generic_node.num_messages_received() != 0);
}
