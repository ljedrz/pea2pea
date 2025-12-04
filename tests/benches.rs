use bytes::{Bytes, BytesMut};
use rand::{Rng, SeedableRng, distr::StandardUniform, rngs::SmallRng};
use tokio::sync::{Barrier, Notify};
use tokio_util::codec::Decoder;

mod common;
use std::{
    io,
    net::SocketAddr,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};

use pea2pea::{
    ConnectionSide, Node, Pea2Pea,
    protocols::{Handshake, OnConnect, OnDisconnect, Reading, Writing},
};

use crate::common::WritingExt;

impl_noop_disconnect_and_handshake!(common::TestNode);

impl_barrier_on_connect!(common::TestNode);

const NUM_MESSAGES: usize = 100_000;
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
struct BenchNode {
    node: Node,
    counter: Arc<AtomicUsize>,
    done: Arc<Notify>,
    expected_count: usize,
}

impl Pea2Pea for BenchNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Reading for BenchNode {
    type Message = ();
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, _src: SocketAddr, _msg: Self::Message) {
        let prev = self.counter.fetch_add(1, Ordering::Relaxed);
        if prev + 1 == self.expected_count {
            self.done.notify_one();
        }
    }
}

async fn run_bench_scenario(sender_count: usize) -> f64 {
    let senders = common::start_test_nodes(sender_count).await;
    let connect_barrier = Arc::new(Barrier::new(sender_count + 1));

    for sender in &senders {
        sender.enable_writing().await;
        sender.barrier.set(connect_barrier.clone()).unwrap();
        sender.enable_on_connect().await;
    }

    let total_messages = sender_count * NUM_MESSAGES;
    let done_notify = Arc::new(Notify::new());

    let receiver = BenchNode {
        node: Node::new(Default::default()),
        counter: Default::default(),
        done: done_notify.clone(),
        expected_count: total_messages,
    };

    receiver.enable_reading().await;
    let receiver_addr = receiver.node().toggle_listener().await.unwrap().unwrap();

    for sender in &senders {
        sender.node().connect(receiver_addr).await.unwrap();
    }

    // wait for connections to stabilize (warmup)
    connect_barrier.wait().await;

    // prepare the Barrier (senders + the main thread)
    let start_barrier = Arc::new(Barrier::new(sender_count + 1));

    // spawn senders
    for sender in senders {
        let barrier = start_barrier.clone();
        tokio::spawn(async move {
            // wait for the gun
            barrier.wait().await;

            // blast
            for i in 0..NUM_MESSAGES {
                // check if this is the last message of a batch
                if (i + 1) % BenchNode::MESSAGE_QUEUE_DEPTH == 0 {
                    // sync point: wait until the Writing task has flushed this message
                    // (and all prior ones) to the socket
                    sender
                        .send_dm(receiver_addr, crate::RANDOM_BYTES.clone())
                        .await
                        .unwrap();
                } else {
                    // fast path: ignore the returned Receiver to avoid overhead, but
                    // unwrap to make sure that the message isn't dropped
                    sender
                        .unicast_fast(receiver_addr, crate::RANDOM_BYTES.clone())
                        .unwrap();
                }
            }
        });
    }

    // start the clock after synchronizing with the senders
    start_barrier.wait().await;
    let start = Instant::now();

    // wait for the "done" signal
    done_notify.notified().await;

    let time_elapsed = start.elapsed().as_millis();
    let bytes_received = receiver.node().stats().received().1;

    (bytes_received as f64) / (time_elapsed as f64 / 1000.0)
}

#[ignore]
#[tokio::test(flavor = "multi_thread")]
async fn bench_spam_to_one() {
    let mut results = Vec::with_capacity(5);
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

    let mut avg_conn_time = std::time::Duration::new(0, 0);
    for _ in 0..NUM_ITERATIONS {
        let initiator = test_node!("initiator");
        let responder = test_node!("responder");
        let conn_barrier = Arc::new(Barrier::new(2));
        responder.barrier.set(conn_barrier.clone()).unwrap();
        responder.enable_on_connect().await;
        let responder_addr = responder.node().toggle_listener().await.unwrap().unwrap();

        let start = std::time::Instant::now();
        initiator.node().connect(responder_addr).await.unwrap();
        conn_barrier.wait().await;
        avg_conn_time += start.elapsed();

        initiator.node().shut_down().await;
        responder.node().shut_down().await;
    }
    avg_conn_time /= NUM_ITERATIONS as u32;

    println!("average connection time: {avg_conn_time:?}\n");
}
