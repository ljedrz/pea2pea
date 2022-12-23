//! An example of simple statistics-driven rate limiting.

mod common;

use std::{io, net::SocketAddr, time::Duration};

use bytes::{Bytes, BytesMut};
use pea2pea::{
    protocols::{Reading, Writing},
    ConnectionSide, Node, Pea2Pea,
};
use rand::{rngs::SmallRng, Rng, SeedableRng};
use tokio::time::sleep;
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

#[derive(Clone)]
struct GenericNode(Node);

impl Pea2Pea for GenericNode {
    fn node(&self) -> &Node {
        &self.0
    }
}

#[async_trait::async_trait]
impl Reading for GenericNode {
    type Message = BytesMut;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    // no extra message processing is required, connection stats are automatic
    async fn process_message(&self, _source: SocketAddr, _message: BytesMut) -> io::Result<()> {
        Ok(())
    }
}

impl Writing for GenericNode {
    type Message = Bytes;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

const NUM_PEERS: usize = 10;

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::INFO);

    // obtain a source of randomness
    let mut rng = SmallRng::from_entropy();

    // start several nodes
    let mut nodes = Vec::with_capacity(NUM_PEERS);
    for i in 0..NUM_PEERS {
        let node = GenericNode(Node::new(Default::default()));
        node.enable_reading().await;
        node.enable_writing().await;

        // the first node will not need to listen for connections
        if i != 0 {
            node.node().start_listening().await.unwrap();
        }

        nodes.push(node);
    }

    // connect the first node to the rest
    for node in nodes.iter().skip(1) {
        nodes[0]
            .node()
            .connect(node.node().listening_addr().unwrap())
            .await
            .unwrap();
    }

    // make sure all the responders have accepted the connection requests
    sleep(Duration::from_millis(100)).await;

    // the first node will be periodically checking for potential spammers
    let recipient = nodes[0].clone();
    tokio::spawn(async move {
        loop {
            // collect info on current connections; keep it linear for simpler tracking
            let conn_infos = recipient
                .node()
                .connection_infos()
                .into_iter()
                .collect::<Vec<_>>();

            // note: in this particular case the recipient node's stats could be used, but a more general solution
            // is to collect info on active connections, as the node's connections can come and go, and its stats
            // accumulate values from older connections as well
            let conn_rates: Vec<(f64, f64)> = conn_infos
                .iter()
                .map(|(_addr, info)| {
                    let lifetime = info.stats().created().elapsed().as_secs();
                    let recv = info.stats().received();

                    let msg_rate = recv.0 as f64 / lifetime as f64;
                    let byte_rate = recv.1 as f64 / lifetime as f64;

                    (msg_rate, byte_rate)
                })
                .collect();

            // calculate average msg/s and bytes/s based on all the connections
            let (sum_msg_rate, sum_byte_rate) = conn_rates.iter().fold(
                (0.0, 0.0),
                |(mut sum_msg_rate, mut sum_byte_rate), (msg_rate, byte_rate)| {
                    sum_msg_rate += msg_rate;
                    sum_byte_rate += byte_rate;

                    (sum_msg_rate, sum_byte_rate)
                },
            );
            let avg_msg_rate = sum_msg_rate / (NUM_PEERS - 1) as f64;
            let avg_byte_rate = sum_byte_rate / (NUM_PEERS - 1) as f64;

            if avg_msg_rate.is_normal() && avg_byte_rate.is_normal() {
                info!(parent: recipient.node().span(), "[spam check] avg. msg/s: {:>6.2}; avg. bytes/s: {:>6.2}", avg_msg_rate, avg_byte_rate);

                // disconnect from nodes with a msg/s rate greater than the average by a factor of 3x or more
                if let Some(idx) = conn_rates
                    .iter()
                    .position(|(msg_rate, _)| *msg_rate > avg_msg_rate * 3.0)
                {
                    warn!(parent: recipient.node().span(), "[spam check] found a potential spammer ({:.2} msg/s, over 3x the average)! disconnecting", conn_rates[idx].0);
                    recipient.node().disconnect(conn_infos[idx].0).await;
                }
            }

            sleep(Duration::from_millis(500)).await;
        }
    });

    // a generic message that the peers of the first node will be sending to it periodically
    let msg = Bytes::from(&b"herp derp"[..]);

    // first node's peers will be sending messages to it with the same frequency, except for the last one skipped here
    for node in nodes.iter().skip(1).take(NUM_PEERS - 2) {
        let node = node.clone();
        let msg = msg.clone();
        tokio::spawn(async move {
            let recipient_addr = node.node().connected_addrs()[0];

            loop {
                node.unicast(recipient_addr, msg.clone())
                    .unwrap()
                    .await
                    .unwrap()
                    .unwrap();

                sleep(Duration::from_millis(100)).await;
            }
        });
    }

    // delay the spam loop by a random value that's small enough to trigger the spam alert
    let delay = rng.gen_range(0..500);
    sleep(Duration::from_millis(delay)).await;

    // the last node will be sending messages with a considerably greater frequency
    let spammer = nodes[nodes.len() - 1].clone();
    tokio::spawn(async move {
        loop {
            let recipient_addr = spammer.node().connected_addrs().get(0).copied();
            if recipient_addr.is_none() {
                warn!(parent: spammer.node().span(), "blast! I've been found!");
                break;
            }

            spammer
                .unicast(recipient_addr.unwrap(), msg.clone())
                .unwrap()
                .await
                .unwrap()
                .unwrap();

            sleep(Duration::from_millis(20)).await;
        }
    });

    // run for a short while
    sleep(Duration::from_secs(5)).await;
}
