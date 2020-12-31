use bytes::Bytes;
use tracing::*;

mod common;
use pea2pea::{
    connect_nodes,
    connections::ConnectionWriter,
    protocols::{Reading, Writing},
    Node, Pea2Pea, Topology,
};

use std::{io, net::SocketAddr, sync::Arc};

#[derive(Clone)]
struct Player(Arc<Node>);

impl Pea2Pea for Player {
    fn node(&self) -> &Arc<Node> {
        &self.0
    }
}

#[async_trait::async_trait]
impl Reading for Player {
    type Message = String;

    fn read_message(
        &self,
        _source: SocketAddr,
        buffer: &[u8],
    ) -> io::Result<Option<(String, usize)>> {
        let bytes = common::read_len_prefixed_message(2, buffer)?;

        Ok(bytes.map(|bytes| (String::from_utf8(bytes[2..].to_vec()).unwrap(), bytes.len())))
    }

    async fn process_message(&self, source: SocketAddr, message: String) -> io::Result<()> {
        info!(
            parent: self.node().span(),
            "{} said \"{}\"{}",
            source,
            message,
            if self.node().name() != "99" { ", passing it on" } else { "" },
        );

        let connected_addrs = self.node().connected_addrs();
        let message_bytes = Bytes::from(message.into_bytes());

        // there are just a maximum of 2 connections, so this is sufficient
        for addr in connected_addrs.into_iter().filter(|addr| *addr != source) {
            self.node()
                .send_direct_message(addr, message_bytes.clone())
                .await
                .unwrap();
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl Writing for Player {
    async fn write_message(&self, writer: &mut ConnectionWriter, payload: &[u8]) -> io::Result<()> {
        let message = crate::common::prefix_with_len(2, payload);

        writer.write_all(&message).await
    }
}

#[tokio::test]
async fn telephone_game() {
    tracing_subscriber::fmt::init();

    let players = common::start_nodes(100, None)
        .await
        .into_iter()
        .map(Player)
        .collect::<Vec<_>>();

    for player in &players {
        player.enable_reading();
        player.enable_writing();
    }
    connect_nodes(&players, Topology::Line).await.unwrap();

    let message = b"when we can't think for ourselves, we can always quote";

    players[0]
        .node()
        .send_direct_message(players[1].node().listening_addr, message[..].into())
        .await
        .unwrap();

    wait_until!(1, players.last().unwrap().node().stats.received().0 == 1);
}
