use bytes::Bytes;
use deadline::deadline;
use rand::{rngs::SmallRng, Rng, SeedableRng};

mod common;
use std::time::Duration;

use pea2pea::{
    protocols::{Reading, Writing},
    Pea2Pea,
};

#[tokio::test]
async fn message_stats() {
    let mut rng = SmallRng::from_entropy();

    let reader = crate::test_node!("reader");
    let reader_addr = reader.node().listening_addr().unwrap();
    reader.enable_reading().await;

    let writer = crate::test_node!("writer");
    writer.enable_writing().await;

    writer.node().connect(reader_addr).await.unwrap();

    let sent_msgs_count = rng.gen_range(2..=64); // shouldn't exceed the outbound queue depth
    let mut msg = vec![0u8; rng.gen_range(1..=4096)];
    rng.fill(&mut msg[..]);

    let msg = Bytes::from(msg);

    for _ in 0..sent_msgs_count {
        writer
            .unicast(reader_addr, msg.clone())
            .unwrap()
            .await
            .unwrap()
            .unwrap();
    }

    // 2 is the common test length prefix size
    let expected_msgs_size = sent_msgs_count * (2 + msg.len() as u64);

    assert_eq!(
        writer.node().stats().sent(),
        (sent_msgs_count, expected_msgs_size)
    );

    deadline!(Duration::from_secs(1), move || {
        if let Some(stats) = writer.node().known_peers().get(reader_addr) {
            let (sent_msgs, sent_bytes) = stats.sent();
            sent_msgs == sent_msgs_count && sent_bytes == expected_msgs_size
        } else {
            false
        }
    });

    let reader_clone = reader.clone();
    deadline!(Duration::from_secs(1), move || reader_clone
        .node()
        .stats()
        .received()
        == (sent_msgs_count, expected_msgs_size));

    deadline!(Duration::from_secs(1), move || {
        if let Some((_, stats)) = reader.node().known_peers().snapshot().iter().next() {
            let (received_msgs, received_bytes) = stats.received();
            received_msgs == sent_msgs_count && received_bytes == expected_msgs_size
        } else {
            false
        }
    });
}
