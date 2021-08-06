use tokio::time::sleep;
use tracing::*;

mod common;
use pea2pea::Pea2Pea;

use std::time::Duration;

impl common::MessagingNode {
    fn perform_periodic_maintenance(&self) {
        let node = self.node().clone();
        tokio::spawn(async move {
            loop {
                debug!(parent: node.span(), "performing maintenance");

                // collect the addresses instead of disconnecting immediately inside the loop,
                // because dropping peers that initiated the connection removes the associated
                // peer stat, which would otherwise lead to a deadlock
                let mut addrs_to_disconnect = Vec::new();

                for addr in node.connected_addrs() {
                    if let Some(stats) = node.known_peers().get(addr).as_deref() {
                        if stats.failures() > 0 {
                            addrs_to_disconnect.push(addr);
                        }
                    }
                }

                for addr in addrs_to_disconnect {
                    node.disconnect(addr).await;
                }

                sleep(Duration::from_millis(10)).await;
            }
        });
    }
}

#[tokio::test]
async fn maintenance_example() {
    // tracing_subscriber::fmt::init();

    let tidy = common::MessagingNode::new("tidyboi").await;
    let rando = common::MessagingNode::new("rando").await;

    tidy.node()
        .connect(rando.node().listening_addr().unwrap())
        .await
        .unwrap();

    tidy.perform_periodic_maintenance();
    tidy.node()
        .known_peers()
        .register_failure(rando.node().listening_addr().unwrap()); // artificially report an issue with rando

    wait_until!(1, tidy.node().num_connected() == 0);
}
