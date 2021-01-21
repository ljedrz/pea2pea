use bytes::Bytes;
use tokio::time::sleep;
use tracing::*;
use tracing_subscriber::filter::{EnvFilter, LevelFilter};

use pea2pea::{
    connect_nodes,
    protocols::{Reading, Writing},
    Node, Pea2Pea, Topology,
};

use std::{convert::TryInto, io, net::SocketAddr, time::Duration};

#[derive(Clone)]
struct Player(Node);

impl Pea2Pea for Player {
    fn node(&self) -> &Node {
        &self.0
    }
}

#[async_trait::async_trait]
impl Reading for Player {
    type Message = String;

    fn read_message(&self, _src: SocketAddr, buffer: &[u8]) -> io::Result<Option<(String, usize)>> {
        if buffer.len() >= 2 {
            let payload_len = u16::from_le_bytes(buffer[..2].try_into().unwrap()) as usize;
            if payload_len == 0 { return Err(io::ErrorKind::InvalidData.into()); }

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

impl Writing for Player {
    fn write_message(&self, _: SocketAddr, payload: &[u8], buffer: &mut [u8]) -> io::Result<usize> {
        buffer[..2].copy_from_slice(&(payload.len() as u16).to_le_bytes());
        buffer[2..][..payload.len()].copy_from_slice(&payload);
        Ok(2 + payload.len())
    }
}

#[tokio::main]
async fn main() {
    let filter = match EnvFilter::try_from_default_env() {
        Ok(filter) => filter.add_directive("mio=off".parse().unwrap()),
        _ => EnvFilter::default()
            .add_directive(LevelFilter::INFO.into())
            .add_directive("mio=off".parse().unwrap()),
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .without_time()
        .with_target(false)
        .init();

    let mut players = Vec::with_capacity(100);
    for _ in 0..100 {
        let player = Player(Node::new(None).await.unwrap());
        players.push(player);
    }

    for player in &players {
        player.enable_reading();
        player.enable_writing();
    }
    connect_nodes(&players, Topology::Line).await.unwrap();

    let message = b"when we can't think for ourselves, we can always quote";

    players[0]
        .node()
        .send_direct_message(players[1].node().listening_addr(), message[..].into())
        .await
        .unwrap();

    while players.last().unwrap().node().stats().received().0 != 1 {
        sleep(Duration::from_millis(10)).await;
    }
}
