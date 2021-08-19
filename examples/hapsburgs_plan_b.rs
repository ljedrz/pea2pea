mod common;

use tokio::time::sleep;
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

use pea2pea::{
    protocols::{Disconnect, Handshake, Reading, Writing},
    Config, Connection, Node, Pea2Pea,
};

use std::{io, net::SocketAddr, time::Duration};

#[derive(Clone)]
struct NakedNode(Node);

impl NakedNode {
    async fn new<T: Into<String>>(name: T) -> Self {
        let config = Config {
            name: Some(name.into()),
            ..Default::default()
        };

        Self(Node::new(Some(config)).await.unwrap())
    }
}

impl Pea2Pea for NakedNode {
    fn node(&self) -> &Node {
        &self.0
    }
}

#[async_trait::async_trait]
impl Handshake for NakedNode {
    async fn perform_handshake(&self, conn: Connection) -> io::Result<Connection> {
        if self.node().name() == "Drebin" {
            sleep(Duration::from_millis(10)).await;
            info!(parent: self.node().span(), "Talk!");
        } else {
            info!(parent: self.node().span(), "<raises hand>");
        }
        Ok(conn)
    }
}

#[async_trait::async_trait]
impl Reading for NakedNode {
    type Message = String;

    fn read_message<R: io::Read>(
        &self,
        _source: SocketAddr,
        reader: &mut R,
    ) -> io::Result<Option<Self::Message>> {
        let vec = common::read_len_prefixed_message::<R, 2>(reader)?;

        vec.map(|v| String::from_utf8(v).map_err(|_| io::ErrorKind::InvalidData.into()))
            .transpose()
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

        self.send_direct_message(source, reply.to_string())
    }
}

impl Writing for NakedNode {
    type Message = String;

    fn write_message<W: io::Write>(
        &self,
        _: SocketAddr,
        payload: &Self::Message,
        buffer: &mut W,
    ) -> io::Result<()> {
        buffer.write_all(&(payload.len() as u16).to_le_bytes())?;
        buffer.write_all(payload.as_bytes())
    }
}

#[async_trait::async_trait]
impl Disconnect for NakedNode {
    async fn handle_disconnect(&self, _addr: SocketAddr) {
        if self.node().name() == "Drebin" {
            info!(parent: self.node().span(), "All right. Who else is almost dead?");
        } else {
            info!(parent: self.node().span(), "<dies>");
        }
    }
}

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::INFO);

    const NUM_THUGS: usize = 10;

    let drebin = NakedNode::new("Drebin").await;
    let drebin_addr = drebin.node().listening_addr().unwrap();

    drebin.enable_handshake();
    drebin.enable_reading();
    drebin.enable_writing();
    drebin.enable_disconnect();

    info!(parent: drebin.node().span(), "Where's Hapsburg?");

    for i in 0..NUM_THUGS {
        let hapsburgs_thug = NakedNode::new(format!("thug {}", i)).await;

        hapsburgs_thug.enable_handshake();
        hapsburgs_thug.enable_reading();
        hapsburgs_thug.enable_writing();
        hapsburgs_thug.enable_disconnect();

        // Habsburg's thugs alert Drebin of their presence
        hapsburgs_thug.node().connect(drebin_addr).await.unwrap();
        sleep(Duration::from_millis(50)).await;
        let thug_addr = drebin.node().connected_addrs()[0];

        drebin
            .send_direct_message(thug_addr, "Talk!".to_string())
            .unwrap();

        sleep(Duration::from_millis(50)).await;

        // the thug dies before revealing the location of Hapsburg's Plan B
        hapsburgs_thug.node().shut_down().await;

        // wait until Drebin realizes the thug is dead
        sleep(Duration::from_millis(50)).await;
    }
}
