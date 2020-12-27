use bytes::Bytes;
use tokio::time::sleep;

use pea2pea::{ContainsNode, Messaging, Node, NodeConfig};

use std::{
    convert::TryInto,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone)]
struct SpamBot(Arc<Node>);

impl ContainsNode for SpamBot {
    fn node(&self) -> &Arc<Node> {
        &self.0
    }
}

#[derive(Clone)]
struct VictimBot(Arc<Node>);

impl ContainsNode for VictimBot {
    fn node(&self) -> &Arc<Node> {
        &self.0
    }
}

#[async_trait::async_trait]
impl Messaging for VictimBot {
    type Message = ();

    fn read_message(buffer: &[u8]) -> Option<&[u8]> {
        // expecting the test messages to be prefixed with their length encoded as a LE u16
        if buffer.len() >= 4 {
            let payload_len = u32::from_le_bytes(buffer[..4].try_into().unwrap()) as usize;

            if buffer[4..].len() >= payload_len {
                Some(&buffer[..4 + payload_len])
            } else {
                None
            }
        } else {
            None
        }
    }

    fn parse_message(&self, _source: SocketAddr, _buffer: &[u8]) -> Option<Self::Message> {
        Some(())
    }
}

fn display_throughput(bytes: f64) {
    const GB: f64 = 1_000_000_000.0;
    const MB: f64 = 1_000_000.0;
    const KB: f64 = 1_000.0;

    if bytes >= GB {
        println!("throughput: {:.2} GB/s", bytes / GB);
    } else if bytes >= MB {
        println!("throughput: {:.2} MB/s", bytes / MB);
    } else if bytes >= KB {
        println!("throughput: {:.2} KB/s", bytes / KB);
    } else {
        println!("throughput: {:.2} B/s", bytes);
    }
}

#[ignore]
#[tokio::test]
async fn bench_spam_to_one() {
    const SPAMMER_COUNT: usize = 1;
    const MSG_COUNT: usize = 100_000;
    const MSG_SIZE: usize = 128;

    let mut config = NodeConfig::default();
    config.outbound_message_queue_depth = MSG_COUNT;
    let spammers = Node::new_multiple(SPAMMER_COUNT, Some(config))
        .await
        .unwrap();
    let spammers = spammers.into_iter().map(SpamBot).collect::<Vec<_>>();

    let mut config = NodeConfig::default();
    config.inbound_message_queue_depth = SPAMMER_COUNT * MSG_COUNT;
    config.conn_read_buffer_size = MSG_SIZE + 4;
    let victim = VictimBot(Node::new(Some(config)).await.unwrap());

    victim.enable_messaging();

    for spammer in &spammers {
        spammer
            .node()
            .initiate_connection(victim.node().listening_addr)
            .await
            .unwrap();
    }
    sleep(Duration::from_millis(100)).await;

    let victim_addr = victim.node().listening_addr;
    let mut msg = vec![0u8; MSG_SIZE + 4];
    let msg_len = (MSG_SIZE as u32).to_le_bytes();
    msg[..4].copy_from_slice(&msg_len);
    let msg = Bytes::from(msg);

    let start = Instant::now();
    for spammer in spammers {
        let msg = msg.clone();
        tokio::spawn(async move {
            for _ in 0..MSG_COUNT {
                spammer
                    .node()
                    .send_direct_message(victim_addr, msg.clone())
                    .await
                    .unwrap();
            }
        });
    }

    while victim.node().num_messages_received() < SPAMMER_COUNT * MSG_COUNT {
        sleep(Duration::from_millis(1)).await;
    }
    let time_elapsed = start.elapsed().as_millis();

    let bytes_received = victim
        .node()
        .known_peers
        .peer_stats()
        .read()
        .values()
        .map(|stats| stats.bytes_received)
        .sum::<u64>();

    let throughput = (bytes_received as f64) / (time_elapsed as f64 / 100.0);
    display_throughput(throughput);
}
