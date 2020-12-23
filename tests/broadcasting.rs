use tokio::time::sleep;
use tracing::*;

mod common;
use pea2pea::{
    spawn_nodes, BroadcastProtocol, ContainsNode, MessagingProtocol, Node, NodeConfig,
    PacketingProtocol,
};

use std::{io, sync::Arc, time::Duration};

#[derive(Clone)]
struct ChattyNode(Arc<Node>);

impl ContainsNode for ChattyNode {
    fn node(&self) -> &Arc<Node> {
        &self.0
    }
}

impl PacketingProtocol for ChattyNode {}

#[async_trait::async_trait]
impl BroadcastProtocol for ChattyNode {
    const INTERVAL_MS: u64 = 100;

    async fn perform_broadcast(&self) -> io::Result<()> {
        let message = "hello there ( ͡° ͜ʖ ͡°)";
        info!(parent: self.node().span(), "sending \"{}\" to all my frens", message);
        self.node()
            .send_broadcast(message.as_bytes().to_vec())
            .await;

        Ok(())
    }
}

#[tokio::test]
async fn broadcast_protocol() {
    tracing_subscriber::fmt::init();

    let generic_nodes = spawn_nodes(4, None)
        .await
        .unwrap()
        .into_iter()
        .map(|node| Arc::new(common::GenericNode(node)))
        .collect::<Vec<_>>();
    for generic in &generic_nodes {
        generic.enable_messaging_protocol();
    }

    let mut broadcaster_config = NodeConfig::default();
    broadcaster_config.name = Some("broadcaster".into());
    let broadcaster = Node::new(Some(broadcaster_config)).await.unwrap();
    let broadcaster = Arc::new(ChattyNode(broadcaster));

    let packeting_closure = Box::new(|message: &[u8]| -> Vec<u8> {
        let mut message_with_u16_len = Vec::with_capacity(message.len() + 2);
        message_with_u16_len.extend_from_slice(&(message.len() as u16).to_le_bytes());
        message_with_u16_len.extend_from_slice(message);
        message_with_u16_len
    });

    broadcaster.enable_packeting_protocol(packeting_closure);
    broadcaster.enable_broadcast_protocol();

    for generic in &generic_nodes {
        broadcaster
            .0
            .initiate_connection(generic.node().listening_addr)
            .await
            .unwrap();
    }

    sleep(Duration::from_millis(100)).await;

    for generic in &generic_nodes {
        assert!(generic.node().num_messages_received() != 0);
    }
}
