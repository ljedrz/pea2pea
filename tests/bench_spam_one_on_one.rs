use tokio::{io::AsyncReadExt, time::sleep};

use pea2pea::{ConnectionReader, ContainsNode, MessagingProtocol, Node, NodeConfig};

use std::{
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

const MSG_COUNT: usize = 1000;
const MSG_SIZE: usize = 1000;

#[async_trait::async_trait]
impl MessagingProtocol for VictimBot {
    type Message = ();

    async fn receive_message(connection_reader: &mut ConnectionReader) -> std::io::Result<&[u8]> {
        connection_reader
            .reader
            .read_exact(&mut connection_reader.buffer[..MSG_SIZE])
            .await?;

        Ok(&connection_reader.buffer[..MSG_SIZE])
    }

    fn parse_message(&self, _source: SocketAddr, _buffer: &[u8]) -> Option<Self::Message> {
        Some(())
    }
}

#[ignore]
#[tokio::test]
async fn bench_spam_one_on_one() {
    let spammer = SpamBot(Node::new(None).await.unwrap());
    let mut config = NodeConfig::default();
    config.inbound_message_queue_depth = MSG_COUNT;
    config.conn_read_buffer_size = MSG_SIZE;
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

    let start = Instant::now();
    for _ in 0..MSG_COUNT {
        spammer
            .node()
            .send_direct_message(victim_addr, None, &msg)
            .await
            .unwrap();
    }
    while victim.node().num_messages_received() < MSG_COUNT {
        sleep(Duration::from_millis(5)).await;
    }

    let spammer_addr = victim.node().handshaken_addrs()[0];
    let time_elapsed = start.elapsed().as_millis();
    let bytes_received = victim
        .node()
        .known_peers
        .peer_stats()
        .read()
        .get(&spammer_addr)
        .unwrap()
        .bytes_received;
    let throughput = (bytes_received as f64 / 1024.0) / (time_elapsed as f64 / 100.0);

    println!("throughput: {:.2} KB/s", throughput);
}
