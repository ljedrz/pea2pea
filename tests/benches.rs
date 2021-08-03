use bytes::Bytes;
use once_cell::sync::Lazy;
use rand::{distributions::Standard, rngs::SmallRng, Rng, SeedableRng};

mod common;
use pea2pea::{
    protocols::{Reading, Writing},
    Node, NodeConfig, Pea2Pea,
};

use std::{io, net::SocketAddr, time::Instant};

static RANDOM_BYTES: Lazy<Bytes> = Lazy::new(|| {
    Bytes::from(
        (&mut SmallRng::from_entropy())
            .sample_iter(Standard)
            .take(64 * 1024 - 4)
            .collect::<Vec<_>>(),
    )
});

#[derive(Clone)]
struct Spammer(Node);

impl Pea2Pea for Spammer {
    fn node(&self) -> &Node {
        &self.0
    }
}

impl Writing for Spammer {
    fn write_message(&self, _: SocketAddr, payload: &[u8], buffer: &mut [u8]) -> io::Result<usize> {
        buffer[..4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        buffer[4..][..payload.len()].copy_from_slice(payload);
        Ok(4 + payload.len())
    }
}

#[derive(Clone)]
struct Sink(Node);

impl Pea2Pea for Sink {
    fn node(&self) -> &Node {
        &self.0
    }
}

impl Reading for Sink {
    type Message = ();

    fn read_message(
        &self,
        _source: SocketAddr,
        buffer: &[u8],
    ) -> io::Result<Option<(Self::Message, usize)>> {
        let bytes = common::read_len_prefixed_message(4, buffer)?;

        Ok(bytes.map(|bytes| ((), bytes.len())))
    }
}

fn display_throughput(bytes: f64) -> String {
    const GB: f64 = 1_000_000_000.0;
    const MB: f64 = 1_000_000.0;
    const KB: f64 = 1_000.0;

    if bytes >= GB {
        format!("{:.2} GB/s", bytes / GB)
    } else if bytes >= MB {
        format!("{:.2} MB/s", bytes / MB)
    } else if bytes >= KB {
        format!("{:.2} KB/s", bytes / KB)
    } else {
        format!("{:.2} B/s", bytes)
    }
}

#[derive(Debug)]
struct BenchParams {
    spammer_count: usize,
    msg_count: usize,
    max_msg_size: usize,
}

async fn run_bench_scenario(sender_count: usize) -> f64 {
    const NUM_MESSAGES: usize = 10_000;
    const MSG_SIZE: usize = 64 * 1024;

    let config = NodeConfig {
        conn_write_buffer_size: MSG_SIZE,
        listener_ip: "127.0.0.1".parse().unwrap(),
        conn_outbound_queue_depth: NUM_MESSAGES,
        ..Default::default()
    };
    let spammers = common::start_nodes(sender_count, Some(config)).await;
    let spammers = spammers.into_iter().map(Spammer).collect::<Vec<_>>();

    for spammer in &spammers {
        spammer.enable_writing();
    }

    let config = NodeConfig {
        conn_read_buffer_size: MSG_SIZE * 3,
        max_connections: sender_count as u16,
        listener_ip: "127.0.0.1".parse().unwrap(),
        ..Default::default()
    };
    let sink = Sink(Node::new(Some(config)).await.unwrap());

    sink.enable_reading();

    for spammer in &spammers {
        spammer
            .node()
            .connect(sink.node().listening_addr())
            .await
            .unwrap();
    }

    wait_until!(10, sink.node().num_connected() == sender_count);

    let sink_addr = sink.node().listening_addr();

    let start = Instant::now();
    for spammer in spammers {
        tokio::spawn(async move {
            for _ in 0..NUM_MESSAGES {
                spammer
                    .node()
                    .send_direct_message(sink_addr, RANDOM_BYTES.clone())
                    .unwrap();
            }
        });
    }

    wait_until!(
        10,
        sink.node().stats().received().0 as usize == sender_count * NUM_MESSAGES
    );

    let time_elapsed = start.elapsed().as_millis();
    let bytes_received = sink.node().stats().received().1;

    (bytes_received as f64) / (time_elapsed as f64 / 100.0)
}

#[ignore]
#[tokio::test(flavor = "multi_thread")]
async fn bench_spam_to_one() {
    let mut results = Vec::with_capacity(4);
    for sender_count in &[1, 10, 20, 50, 100] {
        let throughput = run_bench_scenario(*sender_count).await;
        println!(
            "throughput with {:>3} sender(s), 1 receiver: {}",
            sender_count,
            display_throughput(throughput)
        );
        results.push(throughput);
    }

    let avg_throughput = results.iter().sum::<f64>() / results.len() as f64;
    println!("\naverage: {}", display_throughput(avg_throughput));
}
