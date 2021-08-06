use bytes::Bytes;
use peak_alloc::PeakAlloc;
use tracing::*;

mod common;
use pea2pea::{
    protocols::{Disconnect, Handshaking, Reading, Writing},
    Connection, Node, NodeConfig, Pea2Pea,
};

use std::{io, net::SocketAddr};

#[derive(Clone)]
struct TestNode(Node);

impl TestNode {
    async fn new(name: String) -> Self {
        let config = NodeConfig {
            name: Some(name),
            ..Default::default()
        };

        Self(Node::new(Some(config)).await.unwrap())
    }
}

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

#[async_trait::async_trait]
impl Disconnect for TestNode {
    async fn handle_disconnect(&self, _addr: SocketAddr) {
        if self.node().name() == "Drebin" {
            info!(parent: self.node().span(), "All right. Who else is almost dead?");
        } else {
            info!(parent: self.node().span(), "<dies>");
        }
    }
}

#[tokio::test]
async fn check_node_cleanups() {
    // turn on for a silly commentary
    // tracing_subscriber::fmt::init();

    #[global_allocator]
    static PEAK_ALLOC: PeakAlloc = PeakAlloc;

    const NUM_CONNECTIONS: usize = 100;

    let drebin = TestNode::new("Drebin".into()).await;
    let drebin_addr = drebin.node().listening_addr().unwrap();

    // enable all the protocols to check for any leaks there too
    drebin.enable_handshaking();
    drebin.enable_reading();
    drebin.enable_writing();
    drebin.enable_disconnect();

    let initial_heap_use = PEAK_ALLOC.current_usage_as_kb();

    let mut heap_sizes = vec![initial_heap_use];
    let mut heap_after_1st_conn = 0.0;

    info!(parent: drebin.node().span(), "Where's Hapsburg?");

    for i in 0..NUM_CONNECTIONS {
        let hapsburgs_thug = TestNode::new(format!("thug {}", i)).await;

        hapsburgs_thug.enable_handshaking();
        hapsburgs_thug.enable_reading();
        hapsburgs_thug.enable_writing();
        hapsburgs_thug.enable_disconnect();

        // Habsburg's thugs alert Drebin of their presence; conveniently, it is also the connection
        // direction that allows the collection of `KnownPeers` to remain empty for Drebin
        hapsburgs_thug.node().connect(drebin_addr).await.unwrap();
        wait_until!(1, drebin.node().num_connected() == 1);
        let thug_addr = drebin.node().connected_addrs()[0];

        info!(parent: hapsburgs_thug.node().span(), "<raises hand>");
        info!(parent: drebin.node().span(), "Talk!");
        drebin
            .node()
            .send_direct_message(thug_addr, Bytes::from(&b"Talk!"[..]))
            .unwrap();

        wait_until!(1, hapsburgs_thug.node().stats().sent().0 == 2);

        // the thug dies before revealing the location of Hapsburg's Plan B
        hapsburgs_thug.node().shut_down().await;

        // wait until Drebin realizes the thug is dead
        wait_until!(1, drebin.node().num_connected() == 0);

        let current_heap_size = PEAK_ALLOC.current_usage_as_kb();
        heap_sizes.push(current_heap_size);

        // register heap use once the first connection was established and dropped
        if i == 0 {
            heap_after_1st_conn = current_heap_size;
        }
    }

    // calculate avg heap use
    let avg_heap_use = heap_sizes.iter().sum::<f32>() / heap_sizes.len() as f32;

    // drop the vector of heap sizes so it doesn't affect the results
    drop(heap_sizes);

    // check final heap use and calculate heap growth
    let final_heap_use = PEAK_ALLOC.current_usage_as_kb();
    let heap_growth = (final_heap_use - heap_after_1st_conn) / heap_after_1st_conn * 100.0;

    println!("heap use summary:\n");
    println!("after node setup:      {:.2}kB", initial_heap_use);
    println!("after 1 connection:    {:.2}kB", heap_after_1st_conn);
    println!("after all connections: {:.2}kB", final_heap_use);
    println!();
    println!("average use: {:.2}kB", avg_heap_use);
    println!("maximum use: {:.2}kB", PEAK_ALLOC.peak_usage_as_kb());
    println!("growth:      {:.2}%", heap_growth);

    // regardless of the number of connections the node handles, related memory use
    // shouldn't grow by more than 5%, and locally actually results in around -3.5%
    // even after 10k connects and disconnects, which involve spawning and shutting
    // down temporary nodes
    assert!(heap_growth < 5.0);
}
