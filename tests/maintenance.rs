use tokio::time::sleep;
use tracing::*;

mod common;
use pea2pea::{ContainsNode, MaintenanceProtocol, Node, NodeConfig};

use std::{io, sync::Arc, time::Duration};

#[derive(Clone)]
struct TidyNode(Arc<Node>);

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
async fn maintenance_protocol() {
    tracing_subscriber::fmt::init();

    let rando = common::RandomNode::new("0").await;

    let mut tidy_config = NodeConfig::default();
    tidy_config.name = Some("tidy".into());
    tidy_config.max_allowed_failures = 0;
    let tidy = Node::new(Some(tidy_config)).await.unwrap();
    let tidy = Arc::new(TidyNode(tidy));

    tidy.node()
        .initiate_connection(rando.node().listening_addr)
        .await
        .unwrap();

    tidy.enable_maintenance_protocol();
    tidy.node().register_failure(rando.node().listening_addr); // artificially report an issue with rando
    sleep(Duration::from_millis(100)).await;

    assert_eq!(tidy.node().num_connected(), 0);
}
