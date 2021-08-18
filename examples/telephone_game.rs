mod common;

use bytes::Bytes;
use tokio::time::sleep;
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

use pea2pea::{
    connect_nodes,
    protocols::{Reading, Writing},
    Node, Pea2Pea, Topology,
};

use std::{
    io::{self, Read},
    net::SocketAddr,
    time::Duration,
};

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

    fn read_message<R: io::Read>(
        &self,
        _src: SocketAddr,
        reader: &mut R,
    ) -> io::Result<Option<String>> {
        let mut len_arr = [0u8; 2];
        if reader.read_exact(&mut len_arr).is_err() {
            return Ok(None);
        }
        let payload_len = u16::from_le_bytes(len_arr) as usize;

        if payload_len == 0 {
            return Err(io::ErrorKind::InvalidData.into());
        }

        let mut buffer = vec![0u8; payload_len];
        if reader
            .take(payload_len as u64)
            .read_exact(&mut buffer)
            .is_err()
        {
            Ok(None)
        } else {
            let str = String::from_utf8(buffer).map_err(|_| io::ErrorKind::InvalidData)?;
            Ok(Some(str))
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
            self.send_direct_message(addr, message_bytes.clone())?;
        }

        Ok(())
    }
}

impl Writing for Player {
    fn write_message<W: io::Write>(
        &self,
        _: SocketAddr,
        payload: &[u8],
        writer: &mut W,
    ) -> io::Result<()> {
        writer.write_all(&(payload.len() as u16).to_le_bytes())?;
        writer.write_all(payload)
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
        player.enable_reading();
        player.enable_writing();
    }
    connect_nodes(&players, Topology::Line).await.unwrap();

    let message = "when we can't think for ourselves, we can always quote";

    info!(parent: players[0].node().span(), "psst, player {}; \"{}\", pass it on!", players[1].node().name(), message);
    players[0]
        .send_direct_message(
            players[1].node().listening_addr().unwrap(),
            message.as_bytes().into(),
        )
        .unwrap();

    while players.last().unwrap().node().stats().received().0 != 1 {
        sleep(Duration::from_millis(10)).await;
    }
}
