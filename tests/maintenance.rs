use tokio::time::sleep;
use tracing::*;

mod common;
use pea2pea::{ContainsNode, MaintenanceProtocol, Node, NodeConfig};

use std::{io, ops::Deref, sync::Arc, time::Duration};

#[derive(Clone)]
struct TidyNode(Arc<Node>);

impl Deref for TidyNode {
    type Target = Node;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ContainsNode for TidyNode {
    fn node(&self) -> &Arc<Node> {
        &self.0
    }
}

#[async_trait::async_trait]
impl MaintenanceProtocol for TidyNode {
    const INTERVAL_MS: u64 = 200;

    async fn perform_maintenance(&self) -> io::Result<()> {
        let node = self.node();

        debug!(parent: node.span(), "performing maintenance");

        // collect the addresses instead of disconnecting immediately inside the loop,
        // because dropping peers that initiated the connection removes the associated
        // peer stat, which would otherwise lead to a deadlock
        let mut addrs_to_disconnect = Vec::new();

        for (addr, stats) in node.known_peers.peer_stats().write().iter_mut() {
            if stats.failures > node.config.max_allowed_failures {
                addrs_to_disconnect.push(*addr);
                stats.failures = 0;
            }
        }

        for addr in addrs_to_disconnect {
            node.disconnect(addr);
        }

        Ok(())
    }
}

#[tokio::test]
async fn tidy_node_maintenance() {
    tracing_subscriber::fmt::init();

    let generic_node = common::GenericNode::new().await;

    let mut tidy_node_config = NodeConfig::default();
    tidy_node_config.name = Some("tidy".into());
    tidy_node_config.max_allowed_failures = 0;
    let tidy_node = Node::new(Some(tidy_node_config)).await.unwrap();
    let tidy_node = Arc::new(TidyNode(tidy_node));

    tidy_node
        .node()
        .initiate_connection(generic_node.listening_addr)
        .await
        .unwrap();

    tidy_node.enable_maintenance_protocol();
    tidy_node.register_failure(generic_node.listening_addr);
    sleep(Duration::from_millis(100)).await;

    assert_eq!(tidy_node.node().num_connected(), 0);
}
