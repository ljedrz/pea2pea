use bytes::Bytes;
use tracing::*;

mod common;
use pea2pea::{connect_nodes, Messaging, Node, Pea2Pea, Topology};

use std::{io, net::SocketAddr, sync::Arc};

#[derive(Clone)]
struct Player(Arc<Node>);

impl Pea2Pea for Player {
    fn node(&self) -> &Arc<Node> {
        &self.0
    }
}

#[async_trait::async_trait]
impl Messaging for Player {
    fn read_message(buffer: &[u8]) -> io::Result<Option<&[u8]>> {
        common::read_len_prefixed_message(2, buffer)
    }

    async fn process_message(&self, source: SocketAddr, message: Bytes) -> io::Result<()> {
        // the first 2B are the u16 length
        let message = String::from_utf8(message[2..].to_vec()).unwrap();

        info!(
            parent: self.node().span(),
            "{} said \"{}\"{}",
            source,
            message,
            if self.node().name() != "99" { ", passing it on" } else { "" },
        );

        let connected_addrs = self.node().connected_addrs();

        // there are just a maximum of 2 connections, so this is sufficient
        for addr in connected_addrs.into_iter().filter(|addr| *addr != source) {
            self.node()
                .send_direct_message(addr, common::prefix_with_len(2, message.as_bytes()))
                .await
                .unwrap();
        }

        Ok(())
    }
}

#[tokio::test]
async fn telephone_game() {
    tracing_subscriber::fmt::init();

    let players = common::start_nodes(100, None)
        .await
        .unwrap()
        .into_iter()
        .map(Player)
        .collect::<Vec<_>>();

    // the first node won't be replying to anything, so skip it
    for player in players.iter().skip(1) {
        player.enable_messaging();
    }
    connect_nodes(&players, Topology::Line).await.unwrap();

    let message = "when we can't think for ourselves, we can always quote";

    players[0]
        .node()
        .send_direct_message(
            players[1].node().listening_addr,
            common::prefix_with_len(2, message.as_bytes()),
        )
        .await
        .unwrap();

    wait_until!(1, players.last().unwrap().node().stats.received().0 == 1);
}
