use tokio::time::sleep;
use tracing::*;

mod common;
use pea2pea::{ContainsNode, Node, NodeConfig};

use std::{sync::Arc, time::Duration};

#[derive(Clone)]
struct TidyNode(Arc<Node>);

impl ContainsNode for TidyNode {
    fn node(&self) -> &Arc<Node> {
        &self.0
    }
}

impl TidyNode {
    fn perform_periodic_maintenance(&self) {
        let node = Arc::clone(self.node());
        tokio::spawn(async move {
            loop {
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

                sleep(Duration::from_millis(10)).await;
            }
        });
    }
}

#[tokio::test]
async fn maintenance_example() {
    tracing_subscriber::fmt::init();

    let rando = common::RandomNode::new("0").await;

    let mut tidy_config = NodeConfig::default();
    tidy_config.name = Some("tidy".into());
    tidy_config.max_allowed_failures = 0;
    let tidy = Node::new(Some(tidy_config)).await.unwrap();
    let tidy = TidyNode(tidy);

    tidy.node()
        .initiate_connection(rando.node().listening_addr)
        .await
        .unwrap();

    tidy.perform_periodic_maintenance();
    tidy.node().register_failure(rando.node().listening_addr); // artificially report an issue with rando
    sleep(Duration::from_millis(10)).await;

    assert_eq!(tidy.node().num_connected(), 0);
}
