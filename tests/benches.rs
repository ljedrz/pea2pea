use bytes::{Bytes, BytesMut};
use deadline::deadline;
use once_cell::sync::Lazy;
use rand::{distributions::Standard, rngs::SmallRng, Rng, SeedableRng};
use tokio_util::codec::Decoder;

mod common;
use std::{
    io,
    net::SocketAddr,
    time::{Duration, Instant},
};

use pea2pea::{
    protocols::{Disconnect, Handshake, Reading, Writing},
    ConnectionSide, Node, Pea2Pea,
};

use crate::common::WritingExt;

const NUM_MESSAGES: usize = 10_000;
const MSG_SIZE: usize = 32 * 1024;

static RANDOM_BYTES: Lazy<Bytes> = Lazy::new(|| {
    Bytes::from(
        (&mut SmallRng::from_entropy())
            .sample_iter(Standard)
            .take(MSG_SIZE - 2)
            .collect::<Vec<_>>(),
    )
});

impl Decoder for common::TestCodec<()> {
    type Item = ();
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        Ok(self.0.decode(src)?.map(|_| ()))
    }
}

#[derive(Clone)]
struct BenchNode(Node);

impl Pea2Pea for BenchNode {
    fn node(&self) -> &Node {
        &self.0
    }
}

#[async_trait::async_trait]
impl Reading for BenchNode {
    type Message = ();
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, _src: SocketAddr, _msg: Self::Message) -> io::Result<()> {
        Ok(())
    }
}

async fn run_bench_scenario(sender_count: usize) -> f64 {
    let senders = common::start_test_nodes(sender_count).await;

    for sender in &senders {
        sender.enable_writing().await;
    }

    let receiver = BenchNode(Node::new(Default::default()).await.unwrap());
    receiver.enable_reading().await;

    for sender in &senders {
        sender
            .node()
            .connect(receiver.node().listening_addr().unwrap())
            .await
            .unwrap();
    }

    let receiver_clone = receiver.clone();
    deadline!(Duration::from_secs(10), move || receiver_clone
        .node()
        .num_connected()
        == sender_count);

    let receiver_addr = receiver.node().listening_addr().unwrap();

    let start = Instant::now();
    for sender in senders {
        tokio::spawn(async move {
            for _ in 0..NUM_MESSAGES {
                sender
                    .send_dm(receiver_addr, RANDOM_BYTES.clone())
                    .await
                    .unwrap();
            }
        });
    }

    let receiver_clone = receiver.clone();
    deadline!(
        Duration::from_secs(10),
        move || receiver_clone.node().stats().received().0 as usize == sender_count * NUM_MESSAGES
    );

    let time_elapsed = start.elapsed().as_millis();
    let bytes_received = receiver.node().stats().received().1;

    (bytes_received as f64) / (time_elapsed as f64 / 1000.0)
}

#[ignore]
#[tokio::test(flavor = "multi_thread")]
async fn bench_spam_to_one() {
    let mut results = Vec::with_capacity(4);
    for sender_count in &[1, 10, 20, 50, 100] {
        let throughput = run_bench_scenario(*sender_count).await;
        println!(
            "throughput with {:>3} sender(s), 1 receiver: {}/s",
            sender_count,
            common::display_bytes(throughput)
        );
        results.push(throughput);
    }

    let avg_throughput = results.iter().sum::<f64>() / results.len() as f64;
    println!("\naverage: {}/s\n", common::display_bytes(avg_throughput));
}

#[ignore]
#[tokio::test]
async fn bench_node_startup() {
    const NUM_ITERATIONS: usize = 5000;

    impl_noop_disconnect_and_handshake!(common::TestNode);

    let mut avg_start_up_time = std::time::Duration::new(0, 0);
    for _ in 0..NUM_ITERATIONS {
        let start = std::time::Instant::now();
        let temp_node = crate::test_node!("temp_node");

        temp_node.enable_handshake().await;
        temp_node.enable_reading().await;
        temp_node.enable_writing().await;
        temp_node.enable_disconnect().await;
        avg_start_up_time += start.elapsed();
    }
    avg_start_up_time /= NUM_ITERATIONS as u32;

    println!("average start-up time: {:?}\n", avg_start_up_time);
}

#[ignore]
#[tokio::test]
async fn bench_connection() {
    const NUM_ITERATIONS: usize = 1000;

    let initiator = test_node!("initiator");
    let responder = test_node!("responder");
    let responder_addr = responder.node().listening_addr().unwrap();

    let mut avg_conn_time = std::time::Duration::new(0, 0);
    for _ in 0..NUM_ITERATIONS {
        let start = std::time::Instant::now();
        initiator.node().connect(responder_addr).await.unwrap();
        avg_conn_time += start.elapsed();

        let responder_clone = responder.clone();
        deadline!(Duration::from_secs(1), move || responder_clone
            .node()
            .num_connected()
            == 1);

        initiator.node().disconnect(responder_addr).await;
        let initiator_addr = responder.node().connected_addrs()[0];
        responder.node().disconnect(initiator_addr).await;
    }
    avg_conn_time /= NUM_ITERATIONS as u32;

    println!("average connection time: {:?}\n", avg_conn_time);
}
