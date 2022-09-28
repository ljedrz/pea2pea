use bytes::Bytes;
use deadline::deadline;
use peak_alloc::PeakAlloc;

mod common;
use std::{io, net::SocketAddr, time::Duration};

use pea2pea::{
    protocols::{Disconnect, Handshake, Reading, Writing},
    Pea2Pea,
};

use crate::common::WritingExt;

impl_noop_disconnect_and_handshake!(common::TestNode);

#[tokio::test]
async fn check_node_cleanups() {
    #[global_allocator]
    static PEAK_ALLOC: PeakAlloc = PeakAlloc;

    const NUM_CONNS: usize = 100;

    // register heap use before node setup
    let initial_heap_use = PEAK_ALLOC.current_usage();

    let persistent_node = crate::test_node!("persistent");
    let persistent_addr = persistent_node.node().listening_addr().unwrap();

    // measure the size of a node with no protocols enabled
    let idle_node_size = (PEAK_ALLOC.current_usage() - initial_heap_use) / 1000;

    // enable all the protocols to check for any leaks there too
    persistent_node.enable_handshake().await;
    persistent_node.enable_reading().await;
    persistent_node.enable_writing().await;
    persistent_node.enable_disconnect().await;

    // register heap use after node setup
    let heap_after_node_setup = PEAK_ALLOC.current_usage();

    // start keeping track of the average heap use
    let mut avg_heap_use = 0;

    // due to tokio channel internals, a small heap bump occurs after 32 calls to `mpsc::Sender::send`
    // if it wasn't for that, heap use after the 1st connection (i == 0) would be registered instead
    let mut heap_after_32_conns = 0;

    for i in 0..NUM_CONNS {
        let temp_node = crate::test_node!("temp_node");

        temp_node.enable_handshake().await;
        temp_node.enable_reading().await;
        temp_node.enable_writing().await;
        temp_node.enable_disconnect().await;

        // this connection direction allows the collection of `KnownPeers` to remain empty
        temp_node.node().connect(persistent_addr).await.unwrap();

        let persistent_node_clone = persistent_node.clone();
        let temp_node_clone = temp_node.clone();
        deadline!(Duration::from_secs(1), move || persistent_node_clone
            .node()
            .num_connected()
            == 1
            && temp_node_clone.node().num_connected() == 1);
        let temporary_addr = persistent_node.node().connected_addrs()[0];

        persistent_node
            .send_dm(temporary_addr, Bytes::from(&b"herp"[..]))
            .await
            .unwrap();

        temp_node
            .send_dm(persistent_addr, Bytes::from(&b"derp"[..]))
            .await
            .unwrap();

        temp_node.node().shut_down().await;

        let persistent_node_clone = persistent_node.clone();
        deadline!(Duration::from_secs(1), move || persistent_node_clone
            .node()
            .num_connected()
            == 0);

        // drop the temporary node to fully relinquish its memory
        drop(temp_node);

        // obtain and record current memory use
        let current_heap_use = PEAK_ALLOC.current_usage();

        // register heap use once the 33rd connection is established and dropped
        if i == 32 {
            heap_after_32_conns = current_heap_use;
        }

        // save current heap size to calculate average use later on
        avg_heap_use += current_heap_use;
    }

    // calculate avg heap use
    avg_heap_use /= NUM_CONNS;

    // check final heap use and calculate heap growth
    let final_heap_use = PEAK_ALLOC.current_usage();
    let heap_growth = final_heap_use - heap_after_32_conns;

    // calculate some helper values
    let max_heap_use = PEAK_ALLOC.peak_usage() / 1000;
    let final_heap_use_kb = final_heap_use / 1000;
    let single_node_size = (heap_after_node_setup - initial_heap_use) / 1000;

    println!("---- heap use summary ----\n");
    println!("before node setup:     {}kB", initial_heap_use / 1000);
    println!("after node setup:      {}kB", heap_after_node_setup / 1000);
    println!("after 32 connections:  {}kB", heap_after_32_conns / 1000);
    println!("after {} connections: {}kB", NUM_CONNS, final_heap_use_kb);
    println!("average memory use:    {}kB", avg_heap_use / 1000);
    println!("maximum memory use:    {}kB", max_heap_use); // note: heavily affected by Config::initial_read_buffer_size
    println!();
    println!("idle node size: {}kB", idle_node_size);
    println!("full node size: {}kB", single_node_size);
    println!("leaked memory:  {}B", heap_growth);
    println!();

    // regardless of the number of connections the node handles, its memory use shouldn't grow at all
    assert_eq!(heap_growth, 0);
}
