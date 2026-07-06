//! The classic **C10k problem**: can a single node accept and hold 10,000
//! concurrent connections? This example spawns a swarm of persistent TCP
//! clients that connect to one pea2pea node and keep their sockets open.
//!
//! NOTE: make sure your OS limits are high enough before running - this
//! involves `ulimit -n` (open file descriptors), the ephemeral port range, and
//! SYN-flood protection.

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use pea2pea::{Config, Node, Pea2Pea, protocols::OnConnect};
use tokio::{
    net::TcpStream,
    sync::Notify,
    task::JoinSet,
    time::{sleep, timeout},
};

const TARGET_CONNS: usize = 10_000;

#[derive(Clone)]
struct Server {
    node: Node,
    done: Arc<Notify>,
}

impl Pea2Pea for Server {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl OnConnect for Server {
    async fn on_connect(&self, _addr: SocketAddr) {
        if self.node().num_connected() >= TARGET_CONNS {
            self.done.notify_one();
        }
    }
}

#[tokio::main]
async fn main() {
    // Create the server with a little headroom over the target. The connection
    // limits are `u16`, so saturate at the ceiling rather than silently
    // overflowing (and capping far below the target) if TARGET_CONNS is
    // bumped.
    let cap = (TARGET_CONNS + 1000).min(u16::MAX as usize) as u16;
    let config = Config {
        name: Some("server".into()),
        max_connections: cap,
        max_connections_per_ip: cap,
        max_connecting: cap,
        listener_backlog: 10_000,
        ..Default::default()
    };
    let done = Arc::new(Notify::new());
    let server = Server {
        node: Node::new(config),
        done: done.clone(),
    };
    server.enable_on_connect().await;
    let server_addr = server.node().toggle_listener().await.unwrap().unwrap();

    println!("⚡ Spawning {TARGET_CONNS} persistent clients...");
    let start = Instant::now();

    // These clients do NOT give up; they merely wait and retry. They are raw
    // TCP sockets (not pea2pea nodes), which keeps each client cheap and
    // isolates what we're measuring: the server's inbound accept/hold capacity.
    let mut client_set = JoinSet::new();
    for _ in 0..TARGET_CONNS {
        client_set.spawn(async move {
            loop {
                match TcpStream::connect(server_addr).await {
                    // the binding keeps the socket open and counting towards the limit
                    Ok(_stream) => std::future::pending::<()>().await,
                    Err(_) => {
                        // simulates "backpressure" being handled by the client
                        let jitter = rand::random::<u64>() % 200;
                        sleep(Duration::from_millis(500 + jitter)).await;
                    }
                }
            }
        });
    }

    // wait for the server to register 10k conns
    match timeout(Duration::from_secs(60), done.notified()).await {
        Ok(()) => println!(
            "\n✅ SUCCESS: Reached {TARGET_CONNS} connections in {:.2?}",
            start.elapsed()
        ),
        Err(_) => panic!(
            "\n❌ FAILURE: stalled at {}/{TARGET_CONNS} after 60s",
            server.node().num_connected()
        ),
    }

    // cleanup
    client_set.abort_all();
    server.node().shut_down().await;
}
