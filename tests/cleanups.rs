use bytes::Bytes;
use peak_alloc::PeakAlloc;
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
async fn check_node_cleanups() {
    // turn on for a silly commentary
    // tracing_subscriber::fmt::init();

    #[global_allocator]
    static PEAK_ALLOC: PeakAlloc = PeakAlloc;

    let config = NodeConfig {
        name: Some("Drebin".into()),
        ..Default::default()
    };
    let drebin = TestNode(Node::new(Some(config)).await.unwrap());
    let drebin_addr = drebin.node().listening_addr();

    drebin.enable_handshaking();
    drebin.enable_reading();
    drebin.enable_writing();

    let mut peak_heap = PEAK_ALLOC.peak_usage_as_kb();
    let mut peak_heap_post_1st_conn = 0.0;

    for i in 0u8..255 {
        let hapsburgs_thug = TestNode(Node::new(None).await.unwrap());

        hapsburgs_thug.enable_handshaking();
        hapsburgs_thug.enable_reading();
        hapsburgs_thug.enable_writing();

        // Habsburg's thugs alert Drebin of their presence; conveniently, it is also the connection
        // direction that allows the collection of `KnownPeers` to remain empty for Drebin
        hapsburgs_thug.node().connect(drebin_addr).await.unwrap();
        wait_until!(1, drebin.node().num_connected() == 1);
        let thug_addr = drebin.node().connected_addrs()[0];

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

        // wait until Drebin realizes the thug is dead
        wait_until!(1, drebin.node().num_connected() == 0);

        // check peak heap use, register bumps
        let curr_peak = PEAK_ALLOC.peak_usage_as_kb();
        if curr_peak > peak_heap {
            if i != 0 {
                println!(
                    "heap bump: {:.2}kB at i={} (+{:.2}%)",
                    curr_peak,
                    i,
                    (curr_peak / peak_heap - 1.0) * 100.0
                );
            }
            peak_heap = curr_peak;
        }

        // register peak heap use once the first connection was established and dropped
        if i == 0 {
            peak_heap_post_1st_conn = curr_peak;
        }
    }

    // register peak heap use
    let max_heap_use = PEAK_ALLOC.peak_usage_as_kb();
    println!("peak heap use: {:.2}kB", max_heap_use);

    // even when 4096 Habsburg's thugs show up and die, maximum memory use shouldn't grow
    // by more than 5% (some of which is probably caused by tokio), as the heap bumps are:
    //
    // heap bump: 343.46kB at i=1 (+0.49%)
    // heap bump: 344.84kB at i=2 (+0.40%)
    // heap bump: 355.43kB at i=32 (+3.07%)
    // heap bump: 355.48kB at i=2043 (+0.01%)
    // peak heap use: 355.48kB
    let alloc_growth = max_heap_use / peak_heap_post_1st_conn;
    assert!(alloc_growth < 1.05);
}
