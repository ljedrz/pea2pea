use tokio::time::sleep;
use tracing::*;

mod common;
use pea2pea::{
    BroadcastProtocol, ContainsNode, MessagingProtocol, Node, NodeConfig, PacketingProtocol,
};

use std::{io, sync::Arc, time::Duration};

#[derive(Clone)]
struct ChattyNode(Arc<Node>);

impl ContainsNode for ChattyNode {
    fn node(&self) -> &Arc<Node> {
        &self.0
    }
}

#[async_trait::async_trait]
impl BroadcastProtocol for ChattyNode {
    const INTERVAL_MS: u64 = 50;

    async fn perform_broadcast(&self) -> io::Result<()> {
        if !self.node().handshaken_addrs().is_empty() {
            let message = "hello there ( ͡° ͜ʖ ͡°)";
            info!(parent: self.node().span(), "sending \"{}\" to all my frens", message);
            self.node()
                .send_broadcast(message.as_bytes().to_vec())
                .await;
        } else {
            info!(parent: self.node().span(), "meh, I have no frens to chat with",);
        }

        Ok(())
    }
}

impl PacketingProtocol for ChattyNode {
    fn enable_packeting_protocol(&self) {
        self.node()
            .set_packeting_closure(Box::new(common::packeting_closure));
    }
}

#[tokio::test]
async fn broadcast_protocol() {
    tracing_subscriber::fmt::init();

    let random_nodes = Node::spawn_multiple(4, None)
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

    broadcaster.enable_packeting_protocol();
    broadcaster.enable_broadcast_protocol();

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
