use std::{io, net::SocketAddr, time::Duration};

use pea2pea::{Node, Pea2Pea, protocols::*};

mod noop_node;
pub use noop_node::FullNoopNode;

/// A helper trait to shorten the calls to `Writing::unicast` in tests.
pub trait WritingExt: Writing {
    fn send_dm(
        &self,
        addr: SocketAddr,
        msg: <Self as Writing>::Message,
    ) -> impl std::future::Future<Output = io::Result<()>> + Send {
        async move { self.unicast(addr, msg)?.await.unwrap() }
    }
}

impl<T: Writing> WritingExt for T {}

pub async fn wait_until<F>(within: Duration, mut cond: F)
where
    F: FnMut() -> bool,
{
    let res = tokio::time::timeout(within, async {
        let mut interval = tokio::time::interval(Duration::from_millis(10));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if cond() {
                return;
            }
        }
    })
    .await;
    assert!(res.is_ok(), "condition not satisfied within {within:?}");
}

pub async fn start_listening<T: Pea2Pea>(node: &T) -> SocketAddr {
    node.node().toggle_listener().await.unwrap().unwrap()
}

pub async fn wait_for_connections(node: &Node, n: usize) {
    wait_until(Duration::from_secs(1), || node.num_connected() == n).await;
}

pub fn assert_consistent(node: &Node) {
    let connected = node.connected_addrs();
    let infos = node.connection_infos();
    assert_eq!(node.num_connected(), connected.len());
    assert_eq!(node.num_connected(), infos.len());
    for addr in &connected {
        assert!(node.is_connected(*addr));
        assert!(node.connection_info(*addr).is_some());
        assert!(!node.is_connecting(*addr));
    }
}
