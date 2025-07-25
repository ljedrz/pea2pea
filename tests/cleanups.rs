use bytes::Bytes;
use deadline::deadline;
use peak_alloc::PeakAlloc;

mod common;
use std::{io, net::SocketAddr, time::Duration};

use pea2pea::{
    protocols::{Handshake, OnDisconnect, Reading, Writing},
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

    // measure the size of a node with no functionalities enabled
    let idle_node_size = PEAK_ALLOC.current_usage() - initial_heap_use;

    // enable all the protocols and the listener to check for any leaks there too
    persistent_node.enable_handshake().await;
    persistent_node.enable_reading().await;
    persistent_node.enable_writing().await;
    persistent_node.enable_on_disconnect().await;
    let persistent_addr = persistent_node
        .node()
        .toggle_listener()
        .await
        .unwrap()
        .unwrap();

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
        temp_node.enable_on_disconnect().await;
        temp_node.node().toggle_listener().await.unwrap();

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
    println!("after {NUM_CONNS} connections: {final_heap_use_kb}kB");
    println!("average memory use:    {}kB", avg_heap_use / 1000);
    println!("maximum memory use:    {max_heap_use}kB"); // note: heavily affected by Reading::INITIAL_BUFFER_SIZE
    println!();
    println!("idle node size: {idle_node_size}B");
    println!("full node size: {single_node_size}kB");
    println!("leaked memory:  {heap_growth}B");
    println!();

    // regardless of the number of connections the node handles, its memory use shouldn't grow at all
    assert_eq!(heap_growth, 0);
}
