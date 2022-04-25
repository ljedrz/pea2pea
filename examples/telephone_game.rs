mod common;

use tokio::time::sleep;
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

use pea2pea::{
    connect_nodes,
    protocols::{Reading, Writing},
    Node, Pea2Pea, Topology,
};

use std::{io, net::SocketAddr, time::Duration};

#[derive(Clone)]
struct Player(Node);

impl Pea2Pea for Player {
    fn node(&self) -> &Node {
        &self.0
    }
}

const NUM_PLAYERS: usize = 100;

#[async_trait::async_trait]
impl Reading for Player {
    type Message = String;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, source: SocketAddr, message: String) -> io::Result<()> {
        let own_id = self.node().name().parse::<usize>().unwrap();

        info!(
            parent: self.node().span(),
            "player {} said \"{}\"{}",
            own_id - 1,
            message,
            if own_id != NUM_PLAYERS - 1 { ", passing it on" } else { "" },
        );

        let connected_addrs = self.node().connected_addrs();

        // there are just a maximum of 2 connections, so this is sufficient
        if let Some(addr) = connected_addrs.into_iter().find(|addr| *addr != source) {
            let _ = self.send_direct_message(addr, message)?.await;
        }

        Ok(())
    }
}

impl Writing for Player {
    type Message = String;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr) -> Self::Codec {
        Default::default()
    }
}

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::INFO);

    let mut players = Vec::with_capacity(NUM_PLAYERS);
    for _ in 0..NUM_PLAYERS {
        let player = Player(Node::new(None).await.unwrap());
        players.push(player);
    }

    // technically the first node doesn't need `Reading` and the last one doesn't need `Writing`
    for player in &players {
        player.enable_reading().await;
        player.enable_writing().await;
    }
    connect_nodes(&players, Topology::Line).await.unwrap();

    let message = "when we can't think for ourselves, we can always quote";

    info!(parent: players[0].node().span(), "psst, player {}; \"{}\", pass it on!", players[1].node().name(), message);
    let _ = players[0]
        .send_direct_message(
            players[1].node().listening_addr().unwrap(),
            message.to_string(),
        )
        .unwrap()
        .await;

    while players.last().unwrap().node().stats().received().0 != 1 {
        sleep(Duration::from_millis(10)).await;
    }
}
