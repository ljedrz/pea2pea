use bytes::Bytes;

mod common;
use pea2pea::{
    protocols::{Reading, Writing},
    Pea2Pea,
};

use std::sync::atomic::Ordering::Relaxed;

#[tokio::test]
async fn node_stats() {
    let reader = common::MessagingNode::new("reader").await;
    let reader_addr = reader.node().listening_addr().unwrap();
    reader.enable_reading();

    let writer = common::MessagingNode::new("writer").await;
    writer.enable_writing();

    writer.node().connect(reader_addr).await.unwrap();

    let sent_msgs_count = 64u64; // shouldn't exceed the outbound queue depth
    let msg = Bytes::from(vec![0u8; 3]);

    for _ in 0..sent_msgs_count {
        writer
            .node()
            .send_direct_message(reader_addr, msg.clone())
            .unwrap();
    }

    // 4 is the common test length prefix size
    let expected_msgs_size = sent_msgs_count * (msg.len() as u64 + 4);

    wait_until!(
        1,
        writer.node().stats().sent() == (sent_msgs_count, expected_msgs_size)
    );
    wait_until!(1, {
        if let Some(peer) = writer.node().known_peers().read().values().next() {
            peer.msgs_sent.load(Relaxed) as u64 == sent_msgs_count
                && peer.bytes_sent.load(Relaxed) == expected_msgs_size
        } else {
            false
        }
    });

    wait_until!(
        1,
        reader.node().stats().received() == (sent_msgs_count, expected_msgs_size)
    );
    wait_until!(1, {
        if let Some(peer) = reader.node().known_peers().read().values().next() {
            peer.msgs_received.load(Relaxed) as u64 == sent_msgs_count
                && peer.bytes_received.load(Relaxed) == expected_msgs_size
        } else {
            false
        }
    });
}
