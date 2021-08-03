mod common;

use bytes::Bytes;
use tokio::time::sleep;
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

use pea2pea::{
    connect_nodes,
    protocols::{Reading, Writing},
    Node, NodeConfig, Pea2Pea, Topology,
};

use std::{convert::TryInto, io, net::SocketAddr, time::Duration};

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

    fn read_message(&self, _src: SocketAddr, buffer: &[u8]) -> io::Result<Option<(String, usize)>> {
        if buffer.len() >= 2 {
            let payload_len = u16::from_le_bytes(buffer[..2].try_into().unwrap()) as usize;
            if payload_len == 0 {
                return Err(io::ErrorKind::InvalidData.into());
            }

            if buffer[2..].len() >= payload_len {
                let message = String::from_utf8(buffer[2..][..payload_len].to_vec())
                    .map_err(|_| io::ErrorKind::InvalidData)?;

                Ok(Some((message, 2 + payload_len)))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
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
        let message_bytes = Bytes::from(message.into_bytes());

        // there are just a maximum of 2 connections, so this is sufficient
        for addr in connected_addrs.into_iter().filter(|addr| *addr != source) {
            self.node()
                .send_direct_message(addr, message_bytes.clone())?;
        }

        Ok(())
    }
}

impl Writing for Player {
    fn write_message(&self, _: SocketAddr, payload: &[u8], buffer: &mut [u8]) -> io::Result<usize> {
        buffer[..2].copy_from_slice(&(payload.len() as u16).to_le_bytes());
        buffer[2..][..payload.len()].copy_from_slice(payload);
        Ok(2 + payload.len())
    }
}

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::INFO);

    let mut players = Vec::with_capacity(NUM_PLAYERS);
    let config = NodeConfig {
        listener_ip: "127.0.0.1".parse().unwrap(),
        ..Default::default()
    };
    for _ in 0..NUM_PLAYERS {
        let player = Player(Node::new(Some(config.clone())).await.unwrap());
        players.push(player);
    }

    // technically the first node doesn't need `Reading` and the last one doesn't need `Writing`
    for player in &players {
        player.enable_reading();
        player.enable_writing();
    }
    connect_nodes(&players, Topology::Line).await.unwrap();

    let message = "when we can't think for ourselves, we can always quote";

    info!(parent: players[0].node().span(), "psst, player {}; \"{}\", pass it on!", players[1].node().name(), message);
    players[0]
        .node()
        .send_direct_message(
            players[1].node().listening_addr(),
            message.as_bytes().into(),
        )
        .unwrap();

    while players.last().unwrap().node().stats().received().0 != 1 {
        sleep(Duration::from_millis(10)).await;
    }
}
