use bytes::Bytes;

mod common;
use pea2pea::{
    protocols::{Reading, Writing},
    Node, NodeConfig, Pea2Pea,
};

use std::{io, net::SocketAddr, sync::Arc, time::Instant};

#[derive(Clone)]
struct Spammer(Arc<Node>);

impl Pea2Pea for Spammer {
    fn node(&self) -> &Arc<Node> {
        &self.0
    }
}

impl Writing for Spammer {
    fn write_message(&self, _: SocketAddr, payload: &[u8], buffer: &mut [u8]) -> io::Result<usize> {
        buffer[..4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        buffer[4..][..payload.len()].copy_from_slice(&payload);
        Ok(4 + payload.len())
    }
}

#[derive(Clone)]
struct Sink(Arc<Node>);

impl Pea2Pea for Sink {
    fn node(&self) -> &Arc<Node> {
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

fn display_throughput(bytes: f64) {
    const GB: f64 = 1_000_000_000.0;
    const MB: f64 = 1_000_000.0;
    const KB: f64 = 1_000.0;

    if bytes >= GB {
        println!("\tthroughput: {:.2} GB/s", bytes / GB);
    } else if bytes >= MB {
        println!("\tthroughput: {:.2} MB/s", bytes / MB);
    } else if bytes >= KB {
        println!("\tthroughput: {:.2} KB/s", bytes / KB);
    } else {
        println!("\tthroughput: {:.2} B/s", bytes);
    }
}

#[derive(Debug)]
struct BenchParams {
    spammer_count: usize,
    msg_count: usize,
    msg_size: usize,
    conn_read_buffer_size: usize,
    conn_inbound_queue_depth: usize,
}

impl From<[usize; 5]> for BenchParams {
    fn from(params: [usize; 5]) -> Self {
        Self {
            spammer_count: params[0],
            msg_count: params[1],
            msg_size: params[2],
            conn_read_buffer_size: params[3],
            conn_inbound_queue_depth: params[4],
        }
    }
}

async fn run_bench_scenario(params: BenchParams) -> f64 {
    let BenchParams {
        spammer_count,
        msg_count,
        msg_size,
        conn_read_buffer_size,
        conn_inbound_queue_depth,
    } = params;

    let config = NodeConfig {
        conn_outbound_queue_depth: msg_count,
        conn_write_buffer_size: msg_size,
        ..Default::default()
    };
    let spammers = common::start_nodes(spammer_count, Some(config)).await;
    let spammers = spammers.into_iter().map(Spammer).collect::<Vec<_>>();

    for spammer in &spammers {
        spammer.enable_writing();
    }

    let config = NodeConfig {
        conn_inbound_queue_depth,
        conn_read_buffer_size,
        ..Default::default()
    };
    let sink = Sink(Node::new(Some(config)).await.unwrap());

    sink.enable_reading();

    for spammer in &spammers {
        spammer
            .node()
            .connect(sink.node().listening_addr)
            .await
            .unwrap();
    }

    wait_until!(1, sink.node().num_connected() == spammer_count);

    let sink_addr = sink.node().listening_addr;
    let msg = Bytes::from(vec![0u8; msg_size - 4]); // account for the length prefix

    let start = Instant::now();
    for spammer in spammers {
        let msg = msg.clone();
        tokio::spawn(async move {
            for _ in 0..msg_count {
                spammer
                    .node()
                    .send_direct_message(sink_addr, msg.clone())
                    .await
                    .unwrap();
            }
        });
    }

    wait_until!(
        10,
        sink.node().stats.received().0 as usize == spammer_count * msg_count
    );

    let time_elapsed = start.elapsed().as_millis();
    let bytes_received = sink.node().stats.received().1;

    let throughput = (bytes_received as f64) / (time_elapsed as f64 / 100.0);
    display_throughput(throughput);
    throughput
}

#[ignore]
#[allow(clippy::identity_op)]
#[tokio::test(flavor = "multi_thread")]
async fn bench_spam_to_one() {
    const KIB: usize = 1024;
    const MIB: usize = 1024 * 1024;

    let spammer_counts = [1, 5, 10];
    let msg_sizes = [256, 1 * KIB, 64 * KIB, 1 * MIB];
    let conn_read_buffer_sizes = [1 * MIB, 4 * MIB, 8 * MIB];
    let conn_inbound_queue_depths = [100, 250, 1000];

    let mut scenarios = Vec::new();
    for spammer_count in spammer_counts.iter().copied() {
        for conn_inbound_queue_depth in conn_inbound_queue_depths.iter().copied() {
            for conn_read_buffer_size in conn_read_buffer_sizes.iter().copied() {
                for msg_size in msg_sizes
                    .iter()
                    .filter(|&msg_size| *msg_size <= conn_read_buffer_size)
                    .copied()
                {
                    let msg_count = if msg_size < 64 * KIB {
                        100_000
                    } else if msg_size < 1 * MIB {
                        10_000
                    } else {
                        1000
                    };

                    scenarios.push(BenchParams {
                        spammer_count,
                        msg_count,
                        msg_size,
                        conn_read_buffer_size,
                        conn_inbound_queue_depth,
                    });
                }
            }
        }
    }

    let mut results = Vec::with_capacity(scenarios.len());

    println!("benchmarking {} scenarios", scenarios.len());
    for params in scenarios.into_iter() {
        println!("using {:?}", params);
        let result = run_bench_scenario(params).await;
        results.push(result);
    }

    println!("\naverage:");
    let avg_throughput = results.iter().sum::<f64>() / results.len() as f64;
    display_throughput(avg_throughput);
}
