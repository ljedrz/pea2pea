//!
//! Measures how many complete connection cycles (Connect -> Ping -> Pong -> Disconnect)
//! the system can handle per second.
//!
//! Architecture:
//! - 1 Server Node ( Passive, Echoes data )
//! - N Client Nodes ( Active, Loop: Connect->Send->Wait->Disconnect )

mod common;

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::{Bytes, BytesMut};
use pea2pea::{
    Config, ConnectionSide, Node, Pea2Pea,
    protocols::{Reading, Writing},
};
use tokio::{
    sync::Notify,
    task::JoinSet,
    time::{sleep, timeout},
};
use tracing_subscriber::filter::LevelFilter;

// number of concurrent clients
const CLIENT_COUNT: usize = 100;

// duration of the test
const TEST_DURATION: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct Server {
    node: Node,
}

impl Pea2Pea for Server {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Writing for Server {
    type Message = Bytes;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

impl Reading for Server {
    type Message = BytesMut;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, source: SocketAddr, _message: Self::Message) {
        // the server simply replies, not waiting for the delivery
        let _ = self.unicast(source, Bytes::from_static(b"pong"));
    }
}

#[derive(Clone)]
struct Client {
    node: Node,
    // signal to wake up the loop when a reply arrives
    reply_received: Arc<Notify>,
    // global stats counter shared by all clients
    global_cycles: Arc<AtomicUsize>,
}

impl Pea2Pea for Client {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Writing for Client {
    type Message = Bytes;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

impl Reading for Client {
    type Message = BytesMut;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, _source: SocketAddr, _message: Self::Message) {
        // notify the loop that the round-trip is complete
        self.reply_received.notify_one();
        self.global_cycles.fetch_add(1, Ordering::Relaxed);
    }
}

#[tokio::main]
async fn main() {
    // keep the logs down
    common::start_logger(LevelFilter::WARN);

    println!("--- High-Churn Stress Test ---");
    println!("Spawning 1 Server and {CLIENT_COUNT} distinct Client Nodes.");

    // start the server
    let server_config = Config {
        name: Some("server".into()),
        // bump max connections to handle the rapid turnover
        max_connections: (CLIENT_COUNT * 4) as u16,
        max_connections_per_ip: (CLIENT_COUNT * 4) as u16,
        ..Default::default()
    };
    let server = Server {
        node: Node::new(server_config),
    };
    server.enable_reading().await;
    server.enable_writing().await;
    let server_addr = server.node().toggle_listener().await.unwrap().unwrap();

    // objects shared between the clients
    let global_cycles = Arc::new(AtomicUsize::new(0));
    let running = Arc::new(AtomicBool::new(true));

    // spawn the clients
    let mut client_tasks = JoinSet::new();
    for i in 0..CLIENT_COUNT {
        let cycles = global_cycles.clone();
        let is_running = running.clone();

        client_tasks.spawn(async move {
            let client = Client {
                node: Node::new(Config {
                    name: Some(format!("client_{i}")),
                    ..Default::default()
                }),
                reply_received: Default::default(),
                global_cycles: cycles,
            };
            client.enable_reading().await;
            client.enable_writing().await;

            let msg = Bytes::from_static(b"ping");

            // the stress loop
            while is_running.load(Ordering::Relaxed) {
                // connect to the server
                if client.node.connect(server_addr).await.is_ok() {
                    // send a ping
                    if client.unicast(server_addr, msg.clone()).is_ok() {
                        // wait for pong, but timeout if packet is dropped or we are
                        // shutting down; this prevents the client from hanging forever
                        // if the OS drops a packet during high load
                        match timeout(Duration::from_millis(200), client.reply_received.notified())
                            .await
                        {
                            Ok(_) => {
                                // happy path: received pong, disconnect
                                let _ = client.node.disconnect(server_addr).await;
                            }
                            Err(_) => {
                                // timeout path: packet lost (or shutdown requested);
                                // disconnect to reset state and retry
                                let _ = client.node.disconnect(server_addr).await;
                            }
                        }
                    }
                } else {
                    // if connect fails (e.g. OS ephemeral ports exhausted), back off slightly
                    sleep(Duration::from_millis(10)).await;
                }
            }

            // return the client so we can shut it down in the main thread
            client
        });
    }

    // monitoring
    let start = Instant::now();
    let mut last_snap = start;
    let mut last_count = 0;

    loop {
        sleep(Duration::from_secs(1)).await;

        let now = Instant::now();
        let total = global_cycles.load(Ordering::Relaxed);
        let delta = total - last_count;

        let elapsed_total = now.duration_since(start);
        let elapsed_tick = now.duration_since(last_snap);

        println!(
            "[{:.0}s] Total: {} | Speed: {:.0} cycles/sec | Server Load: {} conns",
            elapsed_total.as_secs_f64(),
            total,
            delta as f64 / elapsed_tick.as_secs_f64(),
            server.node().num_connected()
        );

        last_snap = now;
        last_count = total;

        if elapsed_total >= TEST_DURATION {
            break;
        }
    }

    println!("--- Shutting Down ---");

    // signal the clients tasks to stop running
    running.store(false, Ordering::Relaxed);

    // shut down the clients
    while let Some(res) = client_tasks.join_next().await {
        if let Ok(client) = res {
            client.node.shut_down().await;
        }
    }

    // shut down the server
    server.node().shut_down().await;
}
