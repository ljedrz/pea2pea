use bytes::Bytes;
use tokio::time::sleep;
use tracing::*;

mod common;
use pea2pea::{connect_nodes, ContainsNode, Messaging, Node, Topology};

use std::{convert::TryInto, io, net::SocketAddr, sync::Arc, time::Duration};

#[derive(Clone)]
struct Player(Arc<Node>);

impl ContainsNode for Player {
    fn node(&self) -> &Arc<Node> {
        &self.0
    }
}

fn packet_message(message: &[u8]) -> Bytes {
    let mut bytes = Vec::with_capacity(2 + message.len());
    let u16_len = (message.len() as u16).to_le_bytes();
    bytes.extend_from_slice(&u16_len);
    bytes.extend_from_slice(message);

    bytes.into()
}

#[async_trait::async_trait]
impl Messaging for Player {
    type Message = String;

    fn read_message(buffer: &[u8]) -> io::Result<Option<&[u8]>> {
        // expecting the test messages to be prefixed with their length encoded as a LE u16
        if buffer.len() >= 2 {
            let payload_len = u16::from_le_bytes(buffer[..2].try_into().unwrap()) as usize;

            if payload_len == 0 {
                return Err(io::ErrorKind::InvalidData.into());
            }

            if buffer[2..].len() >= payload_len {
                Ok(Some(&buffer[..2 + payload_len]))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    fn parse_message(&self, _source: SocketAddr, message: Vec<u8>) -> Option<Self::Message> {
        // the first 2B are the u16 length
        String::from_utf8(message[2..].to_vec()).ok()
    }

    async fn respond_to_message(
        &self,
        source: SocketAddr,
        message: Self::Message,
    ) -> io::Result<()> {
        info!(
            parent: self.node().span(),
            "{} said \"{}\"{}",
            source,
            message,
            if self.node().name() != "99" { ", passing it on" } else { "" },
        );

        let connected_addrs = self.node().handshaken_addrs();

        // there are just a maximum of 2 connections, so this is sufficient
        for addr in connected_addrs.into_iter().filter(|addr| *addr != source) {
            self.node()
                .send_direct_message(addr, packet_message(message.as_bytes()))
                .await
                .unwrap();
        }

        Ok(())
    }
}

#[tokio::test]
async fn telephone_game() {
    tracing_subscriber::fmt::init();

    let players = Node::new_multiple(100, None)
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

    sleep(Duration::from_millis(100)).await;

    let message = "when we can't think for ourselves, we can always quote";

    players[0]
        .node()
        .send_direct_message(
            players[1].node().listening_addr,
            packet_message(message.as_bytes()),
        )
        .await
        .unwrap();

    sleep(Duration::from_millis(100)).await;

    assert_eq!(players.last().unwrap().node().num_messages_received(), 1);
}
