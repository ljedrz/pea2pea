use bytes::{Bytes, BytesMut};
use deadline::deadline;
use rand::{Rng, SeedableRng, distr::StandardUniform, rngs::SmallRng};
use tokio_util::codec::Decoder;

mod common;
use std::{
    io,
    net::SocketAddr,
    sync::LazyLock,
    time::{Duration, Instant},
};

use pea2pea::{
    ConnectionSide, Node, Pea2Pea,
    protocols::{Handshake, OnDisconnect, Reading, Writing},
};

use crate::common::WritingExt;

impl_noop_disconnect_and_handshake!(common::TestNode);

const NUM_MESSAGES: usize = 10_000;
const MSG_SIZE: usize = 32 * 1024;

static RANDOM_BYTES: LazyLock<Bytes> = LazyLock::new(|| {
    Bytes::from(
        (&mut SmallRng::from_os_rng())
            .sample_iter(StandardUniform)
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

    let receiver = BenchNode(Node::new(Default::default()));
    receiver.enable_reading().await;
    let receiver_addr = receiver.node().toggle_listener().await.unwrap().unwrap();

    for sender in &senders {
        sender.node().connect(receiver_addr).await.unwrap();
    }

    let receiver_clone = receiver.clone();
    deadline!(Duration::from_secs(10), move || receiver_clone
        .node()
        .num_connected()
        == sender_count);

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
            "throughput with {sender_count:>3} sender(s), 1 receiver: {}/s",
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
    const NUM_ITERATIONS: usize = 1000;

    let mut avg_start_up_time = std::time::Duration::new(0, 0);
    for _ in 0..NUM_ITERATIONS {
        let start = std::time::Instant::now();
        let temp_node = crate::test_node!("temp_node");

        temp_node.enable_handshake().await;
        temp_node.enable_reading().await;
        temp_node.enable_writing().await;
        temp_node.enable_on_disconnect().await;
        temp_node.node().toggle_listener().await.unwrap().unwrap();

        avg_start_up_time += start.elapsed();
    }
    avg_start_up_time /= NUM_ITERATIONS as u32;

    println!("average start-up time: {avg_start_up_time:?}\n");
}

#[ignore]
#[tokio::test]
async fn bench_connection() {
    const NUM_ITERATIONS: usize = 1000;

    let initiator = test_node!("initiator");
    let responder = test_node!("responder");
    let responder_addr = responder.node().toggle_listener().await.unwrap().unwrap();

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

    println!("average connection time: {avg_conn_time:?}\n");
}
