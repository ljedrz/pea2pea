//! The C100k problem: 100,000 concurrent connections on a single machine.
//!
//! A single pea2pea node can't get there on its own - its connection limits are `u16`, so one
//! node tops out at 65,535. The way past that ceiling is to *shard*: run several independent
//! nodes, each comfortably under its own `u16` cap, all bound to the same port via `SO_REUSEPORT`
//! ([`Config::reuse_listener_port`]). The kernel load-balances inbound connections across the
//! shards, and the per-node counts sum to the target. Every shard's limits are derived from the
//! consts below, so the whole topology is described in one place.
//!
//! Reaching 100k actually means defeating *two* ceilings:
//!   - server side: the per-node `u16` connection cap - broken by sharding
//!   - client side: the ephemeral source-port range, which limits a single
//!     `(src_ip, dst_ip:port)` pair to ~64.5k connections - broken by spreading the clients
//!     across several loopback source IPs
//!
//! NOTE: this one genuinely needs a tuned OS. Both ends of every connection live in this process,
//! so 100k connections is ~200k file descriptors - set `ulimit -n` well above that (e.g. 300000).
//! `net.core.somaxconn` must be at least the per-shard backlog (or it's silently clamped), the
//! ephemeral range (`net.ipv4.ip_local_port_range`) should be wide, and SYN-flood protection must
//! not drop the herd. The multi-source-IP trick works out of the box on Linux (all of 127.0.0.0/8
//! is loopback); on macOS you must alias the extra addresses first (`sudo ifconfig lo0 alias
//! 127.0.0.2`, etc.). On an untuned box this will stall - that's an OS-limits problem, not the
//! library.

use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use pea2pea::{Config, Node, Pea2Pea, protocols::OnConnect};
use tokio::{
    net::TcpSocket,
    sync::Notify,
    task::JoinSet,
    time::{sleep, timeout},
};

// --- the whole topology, in one place ---------------------------------------

/// Total connections to reach across all shards.
const TARGET_CONNS: usize = 100_000;
/// Number of independent nodes sharing the listener port.
const SHARDS: usize = 4;
/// Loopback source IPs the clients fan out over (127.0.0.1 ..= 127.0.0.SRC_IPS). Each one gives a
/// fresh 64.5k ephemeral-port space, so SRC_IPS * ~64k must exceed TARGET_CONNS.
const SRC_IPS: usize = 4;

/// Connections each shard is expected to hold (the kernel balances roughly evenly).
const PER_SHARD: usize = TARGET_CONNS.div_ceil(SHARDS);
/// Per-shard connection limits, with a little headroom. Each node tracks its limits
/// independently, so these are *per shard*, not global.
const PER_SHARD_CAP: usize = PER_SHARD + 1_000;

// A single node's limits are `u16` - which is the entire reason we shard. If the per-shard cap
// ever exceeds the ceiling, the `as u16` casts below would silently truncate and the node would
// quietly accept far fewer connections than intended; refuse to compile instead.
const _: () = assert!(
    PER_SHARD_CAP <= u16::MAX as usize,
    "per-shard cap exceeds u16::MAX - raise SHARDS so each node stays under the ceiling",
);

// --- a shard ----------------------------------------------------------------

#[derive(Clone)]
struct Shard {
    node: Node,
    /// shared across every shard, so we can tell when the *aggregate* hits the target
    total: Arc<AtomicUsize>,
    done: Arc<Notify>,
}

impl Pea2Pea for Shard {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl OnConnect for Shard {
    async fn on_connect(&self, _addr: SocketAddr) {
        // count this connection towards the global tally; whichever shard lands the
        // target-th connection wakes the main task. The counter is our own atomic, so this
        // is exact regardless of any per-node ordering.
        if self.total.fetch_add(1, Ordering::Relaxed) + 1 == TARGET_CONNS {
            self.done.notify_one();
        }
    }
}

fn shard_config(addr: SocketAddr, idx: usize) -> Config {
    Config {
        name: Some(format!("shard-{idx}")),
        listener_addr: Some(addr),
        reuse_listener_port: true,
        // per shard; total kernel accept-queue capacity is SHARDS * this
        listener_backlog: PER_SHARD as u32,
        max_connections: PER_SHARD_CAP as u16,
        // limits are per shard, and clients arrive from a handful of source IPs, so each node
        // must allow its full share - PER_SHARD_CAP comfortably covers any single source IP
        max_connections_per_ip: PER_SHARD_CAP as u16,
        max_connecting: PER_SHARD_CAP as u16,
        ..Default::default()
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let total = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(Notify::new());

    let make_shard = |cfg: Config| Shard {
        node: Node::new(cfg),
        total: total.clone(),
        done: done.clone(),
    };

    // Bring up shard 0 on an ephemeral port, then read back the concrete port so the remaining
    // shards can join the *same* SO_REUSEPORT group - port 0 would resolve to a different port
    // for each independent node, and they'd never share a listener.
    let mut shards = Vec::with_capacity(SHARDS);
    let shard0 = make_shard(shard_config((Ipv4Addr::LOCALHOST, 0).into(), 0));
    shard0.enable_on_connect().await;
    let bound = shard0.node().toggle_listener().await.unwrap().unwrap();
    shards.push(shard0);
    println!("📡 {SHARDS} shards sharing {bound} (SO_REUSEPORT)");

    for idx in 1..SHARDS {
        let shard = make_shard(shard_config(bound, idx));
        shard.enable_on_connect().await;
        shard.node().toggle_listener().await.unwrap().unwrap();
        shards.push(shard);
    }

    println!("⚡ Spawning {TARGET_CONNS} persistent clients across {SRC_IPS} source IPs...");
    let start = Instant::now();

    // Raw TCP clients; the kernel routes each to whichever shard. Each client is pinned to one of
    // SRC_IPS loopback source IPs to multiply the ephemeral-port space beyond a single ~64.5k.
    let mut client_set = JoinSet::new();
    for i in 0..TARGET_CONNS {
        let src = Ipv4Addr::new(127, 0, 0, 1 + (i % SRC_IPS) as u8);
        client_set.spawn(async move {
            loop {
                let backoff = || sleep(Duration::from_millis(500 + rand::random::<u64>() % 200));

                let Ok(sock) = TcpSocket::new_v4() else {
                    backoff().await;
                    continue;
                };
                // bind to this client's assigned source IP, ephemeral source port
                if sock.bind((src, 0).into()).is_err() {
                    backoff().await;
                    continue;
                }
                match sock.connect(bound).await {
                    Ok(stream) => {
                        // keep the socket active and counting towards the limit
                        let _ = stream;
                        std::future::pending::<()>().await;
                    }
                    Err(_) => backoff().await,
                }
            }
        });
    }

    // Wait for the aggregate to reach the target, or fail loudly rather than hang forever.
    match timeout(Duration::from_secs(120), done.notified()).await {
        Ok(()) => println!(
            "\n✅ SUCCESS: {TARGET_CONNS} connections across {SHARDS} shards in {:.2?}",
            start.elapsed()
        ),
        Err(_) => panic!(
            "\n❌ FAILURE: stalled at {}/{TARGET_CONNS} after 120s",
            total.load(Ordering::Relaxed)
        ),
    }

    // The kernel should have spread connections roughly evenly across the shards.
    print!("   per-shard split:");
    for (i, shard) in shards.iter().enumerate() {
        print!(" [{i}]={}", shard.node().num_connected());
    }
    println!();

    // Want to verify externally? Uncomment to hold the connections open, then run:
    //   ss -tnH state established "sport = :<port>" | wc -l
    // sleep(Duration::from_secs(15)).await;

    // cleanup
    client_set.abort_all();
    for shard in &shards {
        shard.node().shut_down().await;
    }
}
