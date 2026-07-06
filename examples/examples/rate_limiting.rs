//! An example of simple statistics-driven rate limiting.

use std::{net::SocketAddr, time::Duration};

use bytes::{Bytes, BytesMut};
use pea2pea::{
    Config, ConnectionSide, Node, Pea2Pea,
    protocols::{Reading, Writing},
};
use rand::RngExt;
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

impl Reading for GenericNode {
    type Message = BytesMut;
    type Codec = examples::SimpleCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    // no extra message processing is required, connection stats are automatic
    async fn process_message(&self, _source: SocketAddr, _message: BytesMut) {}
}

impl Writing for GenericNode {
    type Message = Bytes;
    type Codec = examples::SimpleCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

const NUM_PEERS: usize = 10;

#[tokio::main]
async fn main() {
    examples::start_logger(LevelFilter::INFO);

    // obtain a source of randomness
    let mut rng = rand::rng();

    // start several nodes
    let mut nodes = Vec::with_capacity(NUM_PEERS);
    for i in 0..NUM_PEERS {
        // the first node talks to all the (loopback) peers, so it needs per-IP
        // connection headroom (the default limit is 1)
        let config = Config {
            max_connections_per_ip: NUM_PEERS as u16,
            ..Default::default()
        };
        let node = GenericNode(Node::new(config));
        node.enable_reading().await;
        node.enable_writing().await;

        // the first node will not need to listen for connections
        if i != 0 {
            node.node().toggle_listener().await.unwrap();
        }

        nodes.push(node);
    }

    // connect the first node to the rest
    for node in nodes.iter().skip(1) {
        nodes[0]
            .node()
            .connect(node.node().listening_addr().await.unwrap())
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
                    let lifetime = info.stats().created().elapsed().as_millis();
                    let recv = info.stats().received();

                    let msg_rate = recv.0 as f64 / lifetime as f64 * 1000.0;
                    let byte_rate = recv.1 as f64 / lifetime as f64 * 1000.0;

                    (msg_rate, byte_rate)
                })
                .collect();

            // calculate average msg/s and bytes/s based on all the connections
            let sum_msg_rate: f64 = conn_rates.iter().map(|(msg_rate, _)| msg_rate).sum();
            let sum_byte_rate: f64 = conn_rates.iter().map(|(_, byte_rate)| byte_rate).sum();
            let avg_msg_rate = sum_msg_rate / conn_infos.len() as f64;
            let avg_byte_rate = sum_byte_rate / conn_infos.len() as f64;

            if avg_msg_rate.is_normal() && avg_byte_rate.is_normal() {
                info!(
                    parent: recipient.node().span(),
                    "[spam check] avg. msg/s: {:>6.2}; avg. bytes/s: {:>6.2}",
                    avg_msg_rate,
                    avg_byte_rate,
                );

                // disconnect from nodes with a msg/s rate greater than the average by a factor of 3x or more
                if let Some(idx) = conn_rates
                    .iter()
                    .position(|(msg_rate, _)| *msg_rate > avg_msg_rate * 3.0)
                {
                    warn!(
                        parent: recipient.node().span(),
                        "[spam check] found a potential spammer ({:.2} msg/s, over 3x the average)! disconnecting",
                        conn_rates[idx].0,
                    );
                    recipient.node().disconnect(conn_infos[idx].0).await;
                }
            }

            sleep(Duration::from_millis(500)).await;
        }
    });

    // a generic message that the peers of the first node will be sending to it periodically
    let msg = Bytes::from(&b"HerpDerp"[..]);

    // first node's peers will be sending messages to it with the same frequency, except for the last one skipped here
    for node in nodes.iter().skip(1).take(NUM_PEERS - 2) {
        let node = node.clone();
        let msg = msg.clone();
        tokio::spawn(async move {
            let recipient_addr = examples::await_connection(node.node()).await;

            loop {
                // deliberately assert on every layer: queueing, sending, and delivery
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
    let delay = rng.random_range(0..500);
    sleep(Duration::from_millis(delay)).await;

    // the last node will be sending messages with a considerably greater frequency
    let spammer = nodes[nodes.len() - 1].clone();
    tokio::spawn(async move {
        loop {
            let Some(recipient_addr) = spammer.node().connected_addrs().first().copied() else {
                warn!(parent: spammer.node().span(), "blast! I've been found!");
                break;
            };

            spammer
                .unicast(recipient_addr, msg.clone())
                .unwrap()
                .await
                .unwrap()
                .unwrap();

            sleep(Duration::from_millis(20)).await;
        }
    });

    // run for a short while
    sleep(Duration::from_secs(5)).await;

    // nodes are never dropped implicitly; always shut them down once done
    for node in &nodes {
        node.node().shut_down().await;
    }
}
