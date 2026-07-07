//! A group of nodes playing the telephone (AKA Chinese whispers) game.

use std::{net::SocketAddr, time::Duration};

use pea2pea::{
    Config, ConnectionSide, Node, Pea2Pea, Topology, connect_nodes,
    protocols::{Reading, Writing},
};
use rand::RngExt;
use tokio::time::sleep;
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

const NUM_PLAYERS: usize = 20;

#[derive(Clone)]
struct Player(Node);

impl Pea2Pea for Player {
    fn node(&self) -> &Node {
        &self.0
    }
}

impl Reading for Player {
    type Message = String;
    type Codec = examples::SimpleCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, source: SocketAddr, mut message: String) {
        // the player's position is encoded in its (explicitly assigned) name
        let own_id = self.node().name().parse::<usize>().unwrap();

        // "bork" the message with a 50% probability
        if rand::rng().random_bool(0.5) {
            let mut bytes = message.into_bytes();
            let idx = rand::rng().random_range(0..bytes.len());
            let changed_char = rand::rng().random_range(b'a'..=b'z');
            bytes[idx] = changed_char;
            message = String::from_utf8(bytes).unwrap();
            warn!(parent: self.node().span(), "huh? not sure if I got it right, but here goes...");
        }

        info!(
            parent: self.node().span(),
            "player {} said \"{message}\"{}",
            own_id - 1,
            if own_id != NUM_PLAYERS - 1 { ", passing it on" } else { "" },
        );

        let connected_addrs = self.node().connected_addrs();

        // there are just a maximum of 2 connections, so this is sufficient
        if let Some(addr) = connected_addrs.into_iter().find(|addr| *addr != source) {
            // queue -> send -> delivery confirmation; asserting on all three layers
            self.unicast(addr, message).unwrap().await.unwrap().unwrap();
        }
    }
}

impl Writing for Player {
    type Message = String;
    type Codec = examples::SimpleCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

#[tokio::main]
async fn main() {
    examples::start_logger(LevelFilter::INFO);

    let players = (0..NUM_PLAYERS)
        .map(|id| {
            let config = Config {
                name: Some(id.to_string()),
                // a line of loopback nodes needs per-IP connection headroom
                // for the middle players (the default limit is 1)
                max_connections_per_ip: 2,
                ..Default::default()
            };
            Player(Node::new(config))
        })
        .collect::<Vec<_>>();

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

    // wait for the message to make it to the last player (demo-only polling; see
    // the c10k example for the Notify-based completion idiom)
    while players.last().unwrap().node().stats().received().0 != 1 {
        sleep(Duration::from_millis(10)).await;
    }

    // nodes are never dropped implicitly; always shut them down once done
    for player in &players {
        player.node().shut_down().await;
    }
}
