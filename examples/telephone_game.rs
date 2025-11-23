//! A group of nodes playing the telephone (AKA Chinese whispers) game.

mod common;

use std::{io, net::SocketAddr, time::Duration};

use pea2pea::{
    connect_nodes,
    protocols::{Reading, Writing},
    ConnectionSide, Node, Pea2Pea, Topology,
};
use tokio::time::sleep;
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

#[derive(Clone)]
struct Player(Node);

impl Pea2Pea for Player {
    fn node(&self) -> &Node {
        &self.0
    }
}

const NUM_PLAYERS: usize = 100;

impl Reading for Player {
    type Message = String;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, source: SocketAddr, message: String) -> io::Result<()> {
        let own_id = self.node().name().parse::<usize>().unwrap();

        info!(
            parent: self.node().span(),
            "player {} said \"{message}\"{}",
            own_id - 1,
            if own_id != NUM_PLAYERS - 1 { ", passing it on" } else { "" },
        );

        let connected_addrs = self.node().connected_addrs();

        // there are just a maximum of 2 connections, so this is sufficient
        if let Some(addr) = connected_addrs.into_iter().find(|addr| *addr != source) {
            let _ = self.unicast(addr, message)?.await;
        }

        Ok(())
    }
}

impl Writing for Player {
    type Message = String;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::INFO);

    let mut players = Vec::with_capacity(NUM_PLAYERS);
    for _ in 0..NUM_PLAYERS {
        let player = Player(Node::new(Default::default()));
        players.push(player);
    }

    // technically the first node doesn't need `Reading` and the last one doesn't need `Writing`
    for player in &players {
        player.enable_reading().await;
        player.enable_writing().await;
        player.node().toggle_listener().await.unwrap();
    }
    connect_nodes(&players, Topology::Line).await.unwrap();

    let message = "when we can't think for ourselves, we can always quote";

    info!(parent: players[0].node().span(), "psst, player {}; \"{message}\", pass it on!", players[1].node().name());
    let _ = players[0]
        .unicast(
            players[1].node().listening_addr().await.unwrap(),
            message.to_string(),
        )
        .unwrap()
        .await;

    while players.last().unwrap().node().stats().received().0 != 1 {
        sleep(Duration::from_millis(10)).await;
    }
}
