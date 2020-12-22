use tokio::time::sleep;
use tracing::*;

mod common;
use pea2pea::{
    BroadcastProtocol, ContainsNode, MessagingProtocol, Node, NodeConfig, PacketingProtocol,
};

use std::{ops::Deref, sync::Arc, time::Duration};

#[derive(Clone)]
struct ChattyNode(Arc<Node>);

impl Deref for ChattyNode {
    type Target = Node;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ContainsNode for ChattyNode {
    fn node(&self) -> &Arc<Node> {
        &self.0
    }
}

impl PacketingProtocol for ChattyNode {}

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

    let reader_node = common::RwNode::new().await;
    reader_node.enable_messaging_protocol();

    let mut chatty_node_config = NodeConfig::default();
    chatty_node_config.name = Some("chatty".into());
    let chatty_node = Node::new(Some(chatty_node_config)).await.unwrap();
    let chatty_node = Arc::new(ChattyNode(chatty_node));

    let writing_closure = Box::new(|message: &[u8]| -> Vec<u8> {
        let mut message_with_u16_len = Vec::with_capacity(message.len() + 2);
        message_with_u16_len.extend_from_slice(&(message.len() as u16).to_le_bytes());
        message_with_u16_len.extend_from_slice(message);
        message_with_u16_len
    });

    chatty_node.enable_packeting_protocol(writing_closure);

    chatty_node
        .0
        .initiate_connection(reader_node.listening_addr)
        .await
        .unwrap();
    chatty_node.enable_broadcast_protocol();

    sleep(Duration::from_millis(100)).await;

    assert!(reader_node.num_messages_received() != 0);
}
