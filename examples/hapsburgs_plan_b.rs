//! An homage to the Naked Gun trilogy, showing the `OnDisconnect` protocol.

mod common;

use std::{io, net::SocketAddr, time::Duration};

use pea2pea::{
    protocols::{Handshake, OnDisconnect, Reading, Writing},
    Config, Connection, ConnectionSide, Node, Pea2Pea,
};
use tokio::time::sleep;
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

#[derive(Clone)]
struct NakedNode(Node);

impl NakedNode {
    fn new<T: Into<String>>(name: T) -> Self {
        let config = Config {
            name: Some(name.into()),
            ..Default::default()
        };

        Self(Node::new(config))
    }
}

impl Pea2Pea for NakedNode {
    fn node(&self) -> &Node {
        &self.0
    }
}

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

impl Reading for NakedNode {
    type Message = String;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
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

        info!(parent: self.node().span(), "{reply}");

        let _ = self.unicast(source, reply.to_string()).unwrap().await;

        Ok(())
    }
}

impl Writing for NakedNode {
    type Message = String;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

impl OnDisconnect for NakedNode {
    async fn on_disconnect(&self, _addr: SocketAddr) {
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

    let drebin = NakedNode::new("Drebin");
    drebin.enable_handshake().await;
    drebin.enable_reading().await;
    drebin.enable_writing().await;
    drebin.enable_on_disconnect().await;
    let drebin_addr = drebin.node().toggle_listener().await.unwrap().unwrap();

    info!(parent: drebin.node().span(), "Where's Hapsburg?");

    for i in 0..NUM_THUGS {
        let hapsburgs_thug = NakedNode::new(format!("thug {i}"));

        hapsburgs_thug.enable_handshake().await;
        hapsburgs_thug.enable_reading().await;
        hapsburgs_thug.enable_writing().await;
        hapsburgs_thug.enable_on_disconnect().await;

        // Habsburg's thugs alert Drebin of their presence
        hapsburgs_thug.node().connect(drebin_addr).await.unwrap();
        sleep(Duration::from_millis(50)).await;
        let thug_addr = drebin.node().connected_addrs()[0];

        let _ = drebin
            .unicast(thug_addr, "Talk!".to_string())
            .unwrap()
            .await;

        sleep(Duration::from_millis(10)).await;

        // the thug dies before revealing the location of Hapsburg's Plan B
        hapsburgs_thug.node().shut_down().await;
    }
}
