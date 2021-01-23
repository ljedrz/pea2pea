use bytes::Bytes;
use tracing::*;

mod common;
use pea2pea::{
    protocols::{Handshaking, Reading, Writing},
    Connection, Node, NodeConfig, Pea2Pea,
};

use std::{io, net::SocketAddr};

#[derive(Clone)]
struct TestNode(Node);

impl Pea2Pea for TestNode {
    fn node(&self) -> &Node {
        &self.0
    }
}

#[async_trait::async_trait]
impl Handshaking for TestNode {
    async fn perform_handshake(&self, conn: Connection) -> io::Result<Connection> {
        // nothing of interest going on here
        Ok(conn)
    }
}

#[async_trait::async_trait]
impl Reading for TestNode {
    type Message = String;

    fn read_message(
        &self,
        _source: SocketAddr,
        buffer: &[u8],
    ) -> io::Result<Option<(Self::Message, usize)>> {
        let bytes = common::read_len_prefixed_message(2, buffer)?;

        Ok(bytes.map(|bytes| (String::from_utf8(bytes[2..].to_vec()).unwrap(), bytes.len())))
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()> {
        let reply = if self.node().name() == "Drebin" {
            if message == "..." {
                return Ok(());
            } else {
                "Where?"
            }
        } else if self.node().stats().sent().0 == 0 {
            "Hapsburg has Plan B in..."
        } else {
            "..."
        };

        info!(parent: self.node().span(), "{}", reply);

        self.node()
            .send_direct_message(source, Bytes::from(reply))
            .await
    }
}

impl Writing for TestNode {
    fn write_message(&self, _: SocketAddr, payload: &[u8], buffer: &mut [u8]) -> io::Result<usize> {
        buffer[..2].copy_from_slice(&(payload.len() as u16).to_le_bytes());
        buffer[2..][..payload.len()].copy_from_slice(&payload);
        Ok(2 + payload.len())
    }
}

#[tokio::test]
#[ignore]
async fn check_node_cleanups() {
    tracing_subscriber::fmt::init();

    let config = NodeConfig {
        name: Some("Drebin".into()),
        ..Default::default()
    };
    let drebin = TestNode(Node::new(Some(config)).await.unwrap());

    drebin.enable_handshaking();
    drebin.enable_reading();
    drebin.enable_writing();

    loop {
        let hapsburgs_thug = TestNode(Node::new(None).await.unwrap());
        let thug_addr = hapsburgs_thug.node().listening_addr();

        hapsburgs_thug.enable_handshaking();
        hapsburgs_thug.enable_reading();
        hapsburgs_thug.enable_writing();

        drebin.node().connect(thug_addr).await.unwrap();

        wait_until!(1, hapsburgs_thug.node().num_connected() == 1);

        info!(parent: drebin.node().span(), "Talk!");
        drebin
            .node()
            .send_direct_message(thug_addr, Bytes::from(&b"Talk!"[..]))
            .await
            .unwrap();

        wait_until!(1, hapsburgs_thug.node().stats().sent().0 == 2);

        // the thug dies before revealing the location of Hapsburg's Plan B
        hapsburgs_thug.node().shut_down();

        // won't get anything out of this one
        drebin.node().disconnect(thug_addr);

        info!(parent: drebin.node().span(), "All right. Who else is almost dead?");

        wait_until!(1, drebin.node().num_connected() == 0);
    }
}
