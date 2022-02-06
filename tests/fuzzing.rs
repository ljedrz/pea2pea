use circular_queue::CircularQueue;
use rand::{distributions::Standard, rngs::SmallRng, Rng, SeedableRng};

mod common;
use pea2pea::{
    protocols::{Reading, Writing},
    Config, Node, Pea2Pea,
};

use std::time::Instant;

// despite there already being a `cargo-fuzz`-powered test already, this one is much better at
// testing inbound message buffering logic, as its issues are unlikely to cause any panics
#[tokio::test(flavor = "multi_thread")]
async fn fuzzing() {
    // tracing_subscriber::fmt::init();

    let start = Instant::now();

    const MAX_MSG_SIZE: usize = 1024;
    const ITERATIONS: usize = 2_500;

    let config = Config {
        name: Some("receiver".into()),
        read_buffer_size: MAX_MSG_SIZE,
        ..Default::default()
    };
    let receiver = common::MessagingNode(Node::new(Some(config)).await.unwrap());
    receiver.enable_reading().await;
    let receiver_addr = receiver.node().listening_addr().unwrap();

    let config = Config {
        name: Some("sender".into()),
        ..Default::default()
    };
    let sender = common::MessagingNode(Node::new(Some(config)).await.unwrap());
    sender.enable_writing().await;

    sender.node().connect(receiver_addr).await.unwrap();

    wait_until!(1, receiver.node().num_connected() == 1);

    let mut rng = SmallRng::from_entropy();

    let mut expected_msg_count: u64 = 0;
    let mut expected_msg_size: u64 = 0;
    let mut processed_sizes = CircularQueue::with_capacity(256);

    for _ in 0..ITERATIONS {
        let random_len: usize = rng.gen_range(1..=MAX_MSG_SIZE - 2); // account for the length prefix
        let random_payload: Vec<u8> = (&mut rng).sample_iter(Standard).take(random_len).collect();

        sender
            .send_direct_message(receiver_addr, random_payload.into())
            .unwrap()
            .await
            .unwrap();

        processed_sizes.push(random_len);
        expected_msg_count += 1;
        expected_msg_size += 2 + random_len as u64;

        wait_until!(
            1,
            receiver.node().stats().received() == (expected_msg_count, expected_msg_size)
        );

        if receiver.node().num_connected() == 0 {
            let last_processed = processed_sizes.asc_iter().collect::<Vec<_>>();
            panic!("the fuzz test failed! message sizes: {:?}", last_processed);
        }
    }

    let elapsed_secs = start.elapsed().as_millis() as f64 / 1000.0;
    let bytes_per_s = expected_msg_size as f64 / elapsed_secs;

    println!(
        "fuzzing complete; fuzzed {} in {}s ({}/s)",
        common::display_bytes(expected_msg_size as f64),
        elapsed_secs,
        common::display_bytes(bytes_per_s),
    );
}

#[tokio::test]
async fn problem_combination() {
    const MAX_MSG_SIZE: usize = 1024;

    let config = Config {
        name: Some("receiver".into()),
        read_buffer_size: MAX_MSG_SIZE,
        ..Default::default()
    };
    let receiver = common::MessagingNode(Node::new(Some(config)).await.unwrap());
    receiver.enable_reading().await;

    let config = Config {
        name: Some("sender".into()),
        ..Default::default()
    };
    let sender = common::MessagingNode(Node::new(Some(config)).await.unwrap());
    sender.enable_writing().await;

    sender
        .node()
        .connect(receiver.node().listening_addr().unwrap())
        .await
        .unwrap();

    wait_until!(1, receiver.node().num_connected() == 1);

    let mut rng = SmallRng::from_entropy();

    for msg_size in &[
        706, 688, 738, 613, 542, 683, 765, 688, 837, 640, 842, 677, 990, 1011, 706, 877, 877, 718,
        674, 566, 1019, 588, 606, 910, 999, 846, 735, 688, 754, 554, 584,
    ] {
        let random_payload: Vec<u8> = (&mut rng).sample_iter(Standard).take(*msg_size).collect();

        sender
            .send_direct_message(
                receiver.node().listening_addr().unwrap(),
                random_payload.into(),
            )
            .unwrap()
            .await
            .unwrap();

        assert!(
            receiver.node().num_connected() != 0,
            "the fuzz test failed!"
        );
    }
}
