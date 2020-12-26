use tokio::time::sleep;

use pea2pea::{ConnectionReader, ContainsNode, MessagingProtocol, Node, NodeConfig};

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
impl MessagingProtocol for VictimBot {
    type Message = ();

    async fn receive_message(connection_reader: &mut ConnectionReader) -> std::io::Result<&[u8]> {
        // expecting the test messages to be prefixed with their length encoded as a LE u32
        let msg_len = connection_reader.read_bytes(4).await?;
        let msg_len = u32::from_le_bytes(msg_len.try_into().unwrap()) as usize;

        connection_reader.read_bytes(msg_len).await
    }

    fn parse_message(&self, _source: SocketAddr, _buffer: &[u8]) -> Option<Self::Message> {
        Some(())
    }
}

#[ignore]
#[tokio::test]
async fn bench_spam_one_on_one() {
    const MSG_COUNT: usize = 100_000;
    const MSG_SIZE: usize = 128;

    let spammer = SpamBot(Node::new(None).await.unwrap());
    let mut config = NodeConfig::default();
    config.inbound_message_queue_depth = MSG_COUNT;
    config.conn_read_buffer_size = MSG_SIZE + 4;
    let victim = VictimBot(Node::new(Some(config)).await.unwrap());

    victim.enable_messaging_protocol();

    spammer
        .node()
        .initiate_connection(victim.node().listening_addr)
        .await
        .unwrap();
    sleep(Duration::from_millis(100)).await;

    let victim_addr = victim.node().listening_addr;
    let msg = vec![0u8; MSG_SIZE];
    let msg_len = (MSG_SIZE as u32).to_le_bytes();

    let start = Instant::now();
    for _ in 0..MSG_COUNT {
        spammer
            .node()
            .send_direct_message(victim_addr, Some(&msg_len), &msg)
            .await
            .unwrap();
    }
    while victim.node().num_messages_received() < MSG_COUNT {
        sleep(Duration::from_millis(5)).await;
    }
    let time_elapsed = start.elapsed().as_millis();

    let spammer_addr = victim.node().handshaken_addrs()[0];

    let bytes_received = victim
        .node()
        .known_peers
        .peer_stats()
        .read()
        .get(&spammer_addr)
        .unwrap()
        .bytes_received;

    let throughput = (bytes_received as f64) / (time_elapsed as f64 / 100.0);
    display_throughput(throughput);
}

fn display_throughput(bytes: f64) {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const KB: f64 = 1024.0;

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
