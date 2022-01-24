use bytes::Bytes;
use once_cell::sync::Lazy;
use rand::{distributions::Standard, rngs::SmallRng, Rng, SeedableRng};

mod common;
use pea2pea::{
    protocols::{Reading, Writing},
    Config, Node, Pea2Pea,
};

use std::{convert::TryInto, io, net::SocketAddr, time::Instant};

const NUM_MESSAGES: usize = 10_000;
const MSG_SIZE: usize = 64 * 1024;

static RANDOM_BYTES: Lazy<Bytes> = Lazy::new(|| {
    Bytes::from(
        (&mut SmallRng::from_entropy())
            .sample_iter(Standard)
            .take(MSG_SIZE - 4)
            .collect::<Vec<_>>(),
    )
});

#[derive(Clone)]
struct Sink(Node);

impl Pea2Pea for Sink {
    fn node(&self) -> &Node {
        &self.0
    }
}

#[async_trait::async_trait]
impl Reading for Sink {
    type Message = ();

    fn read_message<R: io::Read>(
        &self,
        _source: SocketAddr,
        reader: &mut R,
    ) -> io::Result<Option<Self::Message>> {
        let mut buf = [0u8; MSG_SIZE];

        let payload_len = {
            if reader.read_exact(&mut buf[..4]).is_err() {
                return Ok(None);
            }
            u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize
        };

        if reader.read_exact(&mut buf[..payload_len]).is_err() {
            Ok(None)
        } else {
            Ok(Some(()))
        }
    }

    async fn process_message(&self, _src: SocketAddr, _msg: Self::Message) -> io::Result<()> {
        Ok(())
    }
}

async fn run_bench_scenario(sender_count: usize) -> f64 {
    let config = Config {
        ..Default::default()
    };
    let spammers = common::start_nodes(sender_count, Some(config)).await;
    let spammers = spammers
        .into_iter()
        .map(common::MessagingNode)
        .collect::<Vec<_>>();

    for spammer in &spammers {
        spammer.enable_writing().await;
    }

    let config = Config {
        read_buffer_size: MSG_SIZE * 3,
        max_connections: sender_count as u16,
        ..Default::default()
    };
    let sink = Sink(Node::new(Some(config)).await.unwrap());

    sink.enable_reading().await;

    for spammer in &spammers {
        spammer
            .node()
            .connect(sink.node().listening_addr().unwrap())
            .await
            .unwrap();
    }

    wait_until!(10, sink.node().num_connected() == sender_count);

    let sink_addr = sink.node().listening_addr().unwrap();

    let start = Instant::now();
    for spammer in spammers {
        tokio::spawn(async move {
            for _ in 0..NUM_MESSAGES {
                spammer
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
    println!("\naverage: {}/s", common::display_bytes(avg_throughput));
}
