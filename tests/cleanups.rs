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

        self.node().send_direct_message(source, Bytes::from(reply))
    }
}

impl Writing for TestNode {
    fn write_message(&self, _: SocketAddr, payload: &[u8], buffer: &mut [u8]) -> io::Result<usize> {
        buffer[..2].copy_from_slice(&(payload.len() as u16).to_le_bytes());
        buffer[2..][..payload.len()].copy_from_slice(payload);
        Ok(2 + payload.len())
    }
}

#[tokio::test]
async fn check_node_cleanups() {
    // turn on for a silly commentary
    // tracing_subscriber::fmt::init();

    #[global_allocator]
    static PEAK_ALLOC: PeakAlloc = PeakAlloc;

    const NUM_CONNECTIONS: usize = 1_000;

    let config = NodeConfig {
        name: Some("Drebin".into()),
        listener_ip: "127.0.0.1".parse().unwrap(),
        ..Default::default()
    };
    let drebin = TestNode(Node::new(Some(config)).await.unwrap());
    let drebin_addr = drebin.node().listening_addr();

    drebin.enable_handshaking();
    drebin.enable_reading();
    drebin.enable_writing();

    let mut peak_heap = PEAK_ALLOC.peak_usage_as_kb();
    let mut peak_heap_post_1st_conn = 0.0;

    for i in 0..NUM_CONNECTIONS {
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

    // even when 10k Habsburg's thugs show up and die, maximum memory use shouldn't grow
    // by more than 5% (some of which is probably caused by tokio), as the heap bumps are:
    //
    // heap bump: 354.17kB at i=32 (+3.08%)
    // peak heap use: 354.17kB; total heap growth: 1.0308341
    let alloc_growth = max_heap_use / peak_heap_post_1st_conn;
    println!(
        "peak heap use: {:.2}kB; total heap growth after {} connections: {:.2}",
        max_heap_use, NUM_CONNECTIONS, alloc_growth
    );

    assert!(alloc_growth < 1.1);
}
