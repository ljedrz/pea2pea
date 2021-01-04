use tokio::time::sleep;
use tracing::*;

mod common;
use pea2pea::{Node, NodeConfig, Pea2Pea};

use std::{sync::Arc, time::Duration};

#[derive(Clone)]
struct TidyNode(Arc<Node>);

impl Pea2Pea for TidyNode {
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

                for (addr, stats) in node.known_peers().read().iter() {
                    if stats.failures > 0 {
                        addrs_to_disconnect.push(*addr);
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

    let rando = common::MessagingNode::new("0").await;

    let tidy_config = NodeConfig {
        name: Some("tidy".into()),
        ..Default::default()
    };
    let tidy = Node::new(Some(tidy_config)).await.unwrap();
    let tidy = TidyNode(tidy);

    tidy.node()
        .connect(rando.node().listening_addr)
        .await
        .unwrap();

    tidy.perform_periodic_maintenance();
    tidy.node()
        .known_peers()
        .register_failure(rando.node().listening_addr); // artificially report an issue with rando

    wait_until!(1, tidy.node().num_connected() == 0);
}
