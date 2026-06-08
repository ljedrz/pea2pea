//! Extreme random churn of nodes, connections, and messages, designed to run indefinitely.
//!
//! Multiple worker tasks roll dice and execute random actions against a
//! shared pool of nodes. They race against each other and against
//! in-progress shutdowns. The metrics printer reports totals and per-second
//! rates every few seconds.
//!
//! Run with:
//!     cargo test --release chaos -- --ignored --nocapture
//!
//! Set `CHAOS_SEED=<u64>` in the environment to reproduce a particular run.
//! Note that reproducibility is best-effort - per-worker action sequences
//! are deterministic given the seed, but the interleaving between workers
//! depends on the tokio scheduler and is not.
//!
//! Timeout behavior is selected by `CHAOS_FAST_TIMEOUTS`. Unset (the default)
//! uses realistic connection timeouts, which lets the test reach the saturated,
//! contended regime where concurrency bugs surface; the library degrades
//! gracefully and still cleans up fully there, at the cost of non-deterministic
//! timing. Set, it switches to short timeouts that avoid lock contention and
//! executor starvation, which makes much lower action delays sustainable and the
//! throughput figures more reliably reproducible.
//!
//! ## Tuning
//!
//! For multi-hour unattended runs, the following help avoid OS-level resource
//! pressure, even though the defaults don't strictly require them:
//!
//!     sudo sysctl -w net.ipv4.ip_local_port_range="1024 65535"
//!     sudo sysctl -w net.ipv4.tcp_max_tw_buckets=2000000
//!     sudo sysctl -w net.ipv4.tcp_tw_reuse=1
//!     sudo cpupower frequency-set -g performance
//!
//! For resource-constrained CI runners, reduce MAX_NODES and NUM_WORKERS;
//! the action-mix coverage is preserved and the test still surfaces rare
//! races, just at proportionally lower throughput.

use std::{
    // alloc::System,
    env,
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::{Bytes, BytesMut};
// use heapster::Heapster;
use parking_lot::Mutex;
use pea2pea::{
    Config, Connection, ConnectionSide, Node, Pea2Pea,
    connections::DisconnectOrigin,
    protocols::{Handshake, OnConnect, OnDisconnect, Reading, Writing},
};
use rand::{RngExt, SeedableRng, rngs::SmallRng};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpSocket,
    time::{sleep, timeout},
};
use tokio_util::{codec::LengthDelimitedCodec, sync::CancellationToken};

// =========================================================================
// Knobs
// =========================================================================

/// Maximum number of nodes that may exist in the pool at any one time.
const MAX_NODES: usize = 32;
/// Number of worker tasks issuing random actions in parallel.
const NUM_WORKERS: usize = 16;
/// How often the metrics printer wakes up.
const METRICS_INTERVAL: Duration = Duration::from_secs(5);
/// Per-action sleep range. Tight enough to keep the runtime busy, slack
/// enough to let other workers interleave between actions.
const MIN_ACTION_DELAY_US: u64 = 0;
const MAX_ACTION_DELAY_US: u64 = 500;
/// Message size bounds.
const MIN_MSG_SIZE: usize = 1;
const MAX_MSG_SIZE: usize = 4096;

// Action mix (cumulative thresholds out of 10_000). These values were
// compiled with tokio's survival in mind - too many spawns and shutdowns
// bog down the executor and the OS.
const W_SPAWN: u16 = 10; // 10 pts (0.10%)
const W_SHUTDOWN: u16 = 12; // 2 pts  (0.02%)
const W_CONNECT: u16 = 4000; // 3988 pts (~40%)
const W_DISCONNECT: u16 = 7988; // 3988 pts (~40%)
const W_BROADCAST: u16 = 9000; // 1012 pts (~10%)
// remainder: unicast (~10%)

// Modest spread is enough - 64 sources splits the hash contention 64-way.
// 127.0.0.2 .. 127.0.0.65 (skip .0 and .1, commonly used).
const SRC_IP_COUNT: u32 = 64;
static SRC_IP_CURSOR: AtomicU32 = AtomicU32::new(0);

static MSG_BYTES: &[u8] = &[0xAB; MAX_MSG_SIZE];

// =========================================================================
// Stats
// =========================================================================

struct InFlightGuard<'a>(&'a AtomicUsize);

impl<'a> InFlightGuard<'a> {
    fn new(metric: &'a AtomicUsize) -> Self {
        metric.fetch_add(1, Ordering::Release);
        Self(metric)
    }
}

impl<'a> Drop for InFlightGuard<'a> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

#[derive(Default)]
struct Stats {
    nodes_spawned: AtomicUsize,
    nodes_shutdown: AtomicUsize,
    connects_succeeded: AtomicUsize,
    disconnects: AtomicUsize,
    broadcasts: AtomicUsize,
    unicasts_attempted: AtomicUsize,
    unicasts_succeeded: AtomicUsize,
    msgs_received: AtomicUsize,
    on_connect_fired: AtomicUsize,
    on_disconnect_fired: AtomicUsize,
    err_connect: AtomicUsize,
    err_send: AtomicUsize,
    in_flight_connects: AtomicUsize,
    in_flight_disconnects: AtomicUsize,
    in_flight_spawns: AtomicUsize,
    in_flight_shutdowns: AtomicUsize,
}

#[derive(Default, Clone, Copy)]
struct Snapshot {
    nodes_spawned: usize,
    nodes_shutdown: usize,
    connects_succeeded: usize,
    disconnects: usize,
    broadcasts: usize,
    unicasts_attempted: usize,
    unicasts_succeeded: usize,
    msgs_received: usize,
    on_connect_fired: usize,
    on_disconnect_fired: usize,
    err_connect: usize,
    err_send: usize,
    in_flight_connects: usize,
    in_flight_disconnects: usize,
    in_flight_spawns: usize,
    in_flight_shutdowns: usize,
}

impl Snapshot {
    fn capture(s: &Stats) -> Self {
        Self {
            nodes_spawned: s.nodes_spawned.load(Ordering::Relaxed),
            nodes_shutdown: s.nodes_shutdown.load(Ordering::Relaxed),
            connects_succeeded: s.connects_succeeded.load(Ordering::Relaxed),
            disconnects: s.disconnects.load(Ordering::Relaxed),
            broadcasts: s.broadcasts.load(Ordering::Relaxed),
            unicasts_attempted: s.unicasts_attempted.load(Ordering::Relaxed),
            unicasts_succeeded: s.unicasts_succeeded.load(Ordering::Relaxed),
            msgs_received: s.msgs_received.load(Ordering::Relaxed),
            on_connect_fired: s.on_connect_fired.load(Ordering::Relaxed),
            on_disconnect_fired: s.on_disconnect_fired.load(Ordering::Relaxed),
            err_connect: s.err_connect.load(Ordering::Relaxed),
            err_send: s.err_send.load(Ordering::Relaxed),
            in_flight_connects: s.in_flight_connects.load(Ordering::Acquire),
            in_flight_disconnects: s.in_flight_disconnects.load(Ordering::Acquire),
            in_flight_spawns: s.in_flight_spawns.load(Ordering::Acquire),
            in_flight_shutdowns: s.in_flight_shutdowns.load(Ordering::Acquire),
        }
    }
}

// =========================================================================
// The node
// =========================================================================

#[derive(Clone)]
struct StressNode {
    node: Node,
    stats: Arc<Stats>,
}

impl Pea2Pea for StressNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl StressNode {
    fn new(stats: Arc<Stats>) -> Self {
        let config = Config {
            listener_addr: Some("127.0.0.1:0".parse().unwrap()),
            max_connections: MAX_NODES as u16,
            max_connections_per_ip: MAX_NODES as u16,
            max_connecting: MAX_NODES as u16 / 2,
            connection_timeout_ms: 10,
            ..Default::default()
        };
        Self {
            node: Node::new(config),
            stats,
        }
    }

    async fn install(&self) -> io::Result<()> {
        self.enable_handshake().await;
        self.enable_reading().await;
        self.enable_writing().await;
        self.enable_on_connect().await;
        self.enable_on_disconnect().await;
        self.node.toggle_listener().await?;
        Ok(())
    }
}

impl Handshake for StressNode {
    async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
        let side = conn.side();
        let stream = self.borrow_stream(&mut conn);
        // Trivial two-byte handshake. Asymmetric so a reversed side fails fast.
        match side {
            ConnectionSide::Initiator => {
                stream.write_u8(0xAB).await?;
                let resp = stream.read_u8().await?;
                if resp != 0xCD {
                    return Err(io::Error::other("bad handshake response"));
                }
            }
            ConnectionSide::Responder => {
                let hello = stream.read_u8().await?;
                if hello != 0xAB {
                    return Err(io::Error::other("bad handshake hello"));
                }
                stream.write_u8(0xCD).await?;
            }
        }
        Ok(conn)
    }
}

impl Reading for StressNode {
    type Message = BytesMut;
    type Codec = LengthDelimitedCodec;

    const INITIAL_BUFFER_SIZE: usize = 4 * 1024;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        LengthDelimitedCodec::builder()
            .max_frame_length(MAX_MSG_SIZE)
            .new_codec()
    }

    async fn process_message(&self, _src: SocketAddr, _msg: Self::Message) {
        self.stats.msgs_received.fetch_add(1, Ordering::Relaxed);
    }
}

impl Writing for StressNode {
    type Message = Bytes;
    type Codec = LengthDelimitedCodec;

    const INITIAL_BUFFER_SIZE: usize = 4 * 1024;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        LengthDelimitedCodec::builder()
            .max_frame_length(MAX_MSG_SIZE)
            .new_codec()
    }
}

impl OnConnect for StressNode {
    const ABORTABLE: bool = false;

    async fn on_connect(&self, _addr: SocketAddr) {
        self.stats.on_connect_fired.fetch_add(1, Ordering::Relaxed);
    }
}

impl OnDisconnect for StressNode {
    async fn on_disconnect(&self, _addr: SocketAddr, _origin: DisconnectOrigin) {
        self.stats
            .on_disconnect_fired
            .fetch_add(1, Ordering::Relaxed);
    }
}

// =========================================================================
// Pool helpers
// =========================================================================

type Pool = Arc<Mutex<Vec<StressNode>>>;

fn pick_one(pool: &Pool, rng: &mut SmallRng) -> Option<StressNode> {
    let p = pool.lock();
    if p.is_empty() {
        return None;
    }
    Some(p[rng.random_range(0..p.len())].clone())
}

fn pick_two(pool: &Pool, rng: &mut SmallRng) -> Option<(StressNode, StressNode)> {
    let p = pool.lock();
    if p.len() < 2 {
        return None;
    } else if p.len() == 2 {
        let i = rng.random_range(0..2);
        return Some((p[i].clone(), p[1 - i].clone()));
    }
    let i = rng.random_range(0..p.len());
    let mut j = rng.random_range(0..(p.len() - 1));
    if j >= i {
        j += 1;
    }
    Some((p[i].clone(), p[j].clone()))
}

fn pop_random(pool: &Pool, rng: &mut SmallRng) -> Option<StressNode> {
    let mut p = pool.lock();
    if p.is_empty() {
        return None;
    }
    let idx = rng.random_range(0..p.len());
    Some(p.swap_remove(idx))
}

fn random_message(rng: &mut SmallRng) -> Bytes {
    let size = rng.random_range(MIN_MSG_SIZE..MAX_MSG_SIZE);
    Bytes::from_static(&MSG_BYTES[..size])
}

// =========================================================================
// Actions
// =========================================================================

async fn act_spawn(pool: &Pool, stats: &Arc<Stats>, token: &CancellationToken) {
    // cheap pre-check; the real check happens under the lock below
    if pool.lock().len() >= MAX_NODES {
        return;
    }

    let _guard = InFlightGuard::new(&stats.in_flight_spawns);
    let node = StressNode::new(stats.clone());

    tokio::select! {
        biased;
        _ = token.cancelled() => {},
        res = node.install() => if res.is_ok() {
            let pushed = {
                let mut pool = pool.lock();
                if !token.is_cancelled() && pool.len() < MAX_NODES {
                    pool.push(node.clone());
                    stats.nodes_spawned.fetch_add(1, Ordering::Relaxed);
                    true
                } else {
                    false
                }
            };
            if !pushed {
                node.node.shut_down().await;
            }
        }
    }
}

async fn act_shutdown(pool: &Pool, stats: &Arc<Stats>, rng: &mut SmallRng) {
    if let Some(node) = pop_random(pool, rng) {
        let _guard = InFlightGuard::new(&stats.in_flight_shutdowns);
        node.node().shut_down().await;
        stats.nodes_shutdown.fetch_add(1, Ordering::Relaxed);
        // dropping `node` here releases the local Arc clone; if no worker is
        // mid-action on it, the InnerNode Arc count goes to zero shortly
    }
}

async fn act_connect(
    pool: &Pool,
    stats: &Arc<Stats>,
    rng: &mut SmallRng,
    token: &CancellationToken,
    fast_timeouts: bool,
) {
    fn next_source_addr() -> SocketAddr {
        let n = SRC_IP_CURSOR.fetch_add(1, Ordering::Relaxed) % SRC_IP_COUNT;
        let octet = (n + 2) as u8;
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, octet)), 0)
    }

    let Some((a, b)) = pick_two(pool, rng) else {
        return;
    };
    let Ok(target) = b.node().listening_addr().await else {
        return;
    };

    if token.is_cancelled() {
        return;
    }

    let socket = match TcpSocket::new_v4() {
        Ok(s) => s,
        Err(_) => {
            stats.err_connect.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };
    // SO_REUSEADDR isn't necessary here since each source IP has its own
    // ephemeral port pool; SO_REUSEPORT/IP_BIND_ADDRESS_NO_PORT also unneeded.
    if socket.bind(next_source_addr()).is_err() {
        stats.err_connect.fetch_add(1, Ordering::Relaxed);
        return;
    }

    let _guard = InFlightGuard::new(&stats.in_flight_connects);
    tokio::select! {
        biased;
        _ = token.cancelled() => {},
        success = async move {
            if fast_timeouts {
                timeout(Duration::from_secs(1), a.node().connect_using_socket(target, socket)).await.is_ok_and(|r| r.is_ok())
            } else {
                a.node().connect_using_socket(target, socket).await.is_ok()
            }
        } => {
            if success {
                stats.connects_succeeded.fetch_add(1, Ordering::Relaxed);
            } else {
                stats.err_connect.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

async fn act_disconnect(pool: &Pool, stats: &Arc<Stats>, rng: &mut SmallRng) {
    let Some(a) = pick_one(pool, rng) else { return };
    let peers = a.node().connected_addrs();
    if peers.is_empty() {
        return;
    }
    let target = peers[rng.random_range(0..peers.len())];
    let _guard = InFlightGuard::new(&stats.in_flight_disconnects);
    if a.node().disconnect(target).await {
        stats.disconnects.fetch_add(1, Ordering::Relaxed);
    }
}

async fn act_broadcast(pool: &Pool, stats: &Arc<Stats>, rng: &mut SmallRng) {
    let Some(a) = pick_one(pool, rng) else { return };
    let msg = random_message(rng);
    match a.broadcast(msg) {
        Ok(_) => {
            stats.broadcasts.fetch_add(1, Ordering::Relaxed);
        }
        Err(_) => {
            stats.err_send.fetch_add(1, Ordering::Relaxed);
        }
    }
}

async fn act_unicast(pool: &Pool, stats: &Arc<Stats>, rng: &mut SmallRng) {
    let Some(a) = pick_one(pool, rng) else { return };
    let peers = a.node().connected_addrs();
    if peers.is_empty() {
        return;
    }
    let target = peers[rng.random_range(0..peers.len())];
    let msg = random_message(rng);
    stats.unicasts_attempted.fetch_add(1, Ordering::Relaxed);
    match a.unicast_fast(target, msg) {
        Ok(_) => {
            stats.unicasts_succeeded.fetch_add(1, Ordering::Relaxed);
        }
        Err(_) => {
            stats.err_send.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// =========================================================================
// Worker
// =========================================================================

async fn worker(
    pool: Pool,
    stats: Arc<Stats>,
    token: CancellationToken,
    mut rng: SmallRng,
    fast_timeouts: bool,
) {
    while !token.is_cancelled() {
        let roll: u16 = rng.random_range(..10_000);
        if roll < W_SPAWN {
            act_spawn(&pool, &stats, &token).await;
        } else if roll < W_SHUTDOWN {
            act_shutdown(&pool, &stats, &mut rng).await;
        } else if roll < W_CONNECT {
            act_connect(&pool, &stats, &mut rng, &token, fast_timeouts).await;
        } else if roll < W_DISCONNECT {
            act_disconnect(&pool, &stats, &mut rng).await;
        } else if roll < W_BROADCAST {
            act_broadcast(&pool, &stats, &mut rng).await;
        } else {
            act_unicast(&pool, &stats, &mut rng).await;
        }

        if token.is_cancelled() {
            break;
        }

        let sleep_us = rng.random_range(MIN_ACTION_DELAY_US..=MAX_ACTION_DELAY_US);
        tokio::select! {
            _ = sleep(Duration::from_micros(sleep_us)) => {}
            _ = token.cancelled() => break,
        }
    }
}

// =========================================================================
// Metrics
// =========================================================================

fn print_metrics(start: Instant, alive: usize, cur: &Snapshot, prev: &Snapshot) {
    let elapsed = start.elapsed().as_secs_f64();
    let dt = METRICS_INTERVAL.as_secs_f64();
    let rate = |c: usize, p: usize| (c.saturating_sub(p)) as f64 / dt;

    println!(
        "[t={elapsed:>5.0}s] alive={alive:>2}/{max} | \
         nodes spawned/shut={ns}/{nd} | \
         conn att/ok/err={ca}/{cs}/{ce} | disc={dc} | \
         bcast={bc} ucast att/ok={ua}/{us} send-err={se} | \
         recv={rv} | on_c/on_d={oc}/{od} | \
         ifc={ifc} | ifdc={ifdc} | ifsp={ifsp} | ifsd={ifsd}",
        max = MAX_NODES,
        ns = cur.nodes_spawned,
        nd = cur.nodes_shutdown,
        ca = cur.connects_succeeded + cur.err_connect,
        cs = cur.connects_succeeded,
        ce = cur.err_connect,
        dc = cur.disconnects,
        bc = cur.broadcasts,
        ua = cur.unicasts_attempted,
        us = cur.unicasts_succeeded,
        se = cur.err_send,
        rv = cur.msgs_received,
        oc = cur.on_connect_fired,
        od = cur.on_disconnect_fired,
        ifc = cur.in_flight_connects,
        ifdc = cur.in_flight_disconnects,
        ifsp = cur.in_flight_spawns,
        ifsd = cur.in_flight_shutdowns,
    );
    println!(
        "           Δ/s: conn={:.1} disc={:.1} bcast={:.1} ucast={:.1} recv={:.1}",
        rate(cur.connects_succeeded, prev.connects_succeeded),
        rate(cur.disconnects, prev.disconnects),
        rate(cur.broadcasts, prev.broadcasts),
        rate(cur.unicasts_succeeded, prev.unicasts_succeeded),
        rate(cur.msgs_received, prev.msgs_received),
    );
}

// =========================================================================
// Test entry point
// =========================================================================

// #[global_allocator]
// static GLOBAL: Heapster<System> = Heapster::new(System);

#[test]
#[ignore = "long-running stress test"]
fn infinite_chaos() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .global_queue_interval(3)
        .event_interval(31)
        .build()
        .unwrap();

    rt.block_on(infinite_chaos_inner());
}

async fn infinite_chaos_inner() {
    // Determine and log the master seed. CHAOS_SEED overrides for reruns.
    let master_seed: u64 = env::var("CHAOS_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(rand::random);
    let runtime: Duration = env::var("CHAOS_RUNTIME_SECS")
        .ok()
        .and_then(|s| s.parse().ok().map(Duration::from_secs))
        .unwrap_or(Duration::MAX);
    let fast_timeouts = env::var("CHAOS_FAST_TIMEOUTS")
        .map(|v| matches!(v.trim(), "1" | "true" | "yes"))
        .unwrap_or(false);

    println!(
        "pea2pea stress test - {NUM_WORKERS} workers, up to {MAX_NODES} nodes\n\
         seed: {master_seed}\n"
    );

    let mut master_rng = SmallRng::seed_from_u64(master_seed);
    let stats: Arc<Stats> = Default::default();
    let pool: Pool = Default::default();
    let token = CancellationToken::new();

    // Seed the pool with two nodes so workers have something to do immediately.
    for _ in 0..2 {
        let n = StressNode::new(stats.clone());
        n.install().await.unwrap();
        stats.nodes_spawned.fetch_add(1, Ordering::Relaxed);
        pool.lock().push(n);
    }

    // Spawn workers, each with a derived RNG seeded from the master.
    let mut workers = Vec::with_capacity(NUM_WORKERS);
    for _ in 0..NUM_WORKERS {
        let worker_seed: u64 = master_rng.random();
        let worker_rng = SmallRng::seed_from_u64(worker_seed);
        workers.push(tokio::spawn(worker(
            pool.clone(),
            stats.clone(),
            token.clone(),
            worker_rng,
            fast_timeouts,
        )));
    }

    // Spawn the metrics printer.
    let m_stats = stats.clone();
    let m_pool = pool.clone();
    let m_token = token.clone();
    let start = Instant::now();
    let _metrics = tokio::spawn(async move {
        let mut prev = Snapshot::default();
        loop {
            tokio::select! {
                _ = sleep(METRICS_INTERVAL) => {
                    let snap = Snapshot::capture(&m_stats);
                    let alive = m_pool.lock().len();
                    print_metrics(start, alive, &snap, &prev);
                    prev = snap;
                }
                _ = m_token.cancelled() => break,
            }
        }
    });

    // Wait for the deadline to expire or for Ctrl-C.
    tokio::select! {
        _ = sleep(runtime) => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    token.cancel();

    println!("\nShutting down...\n");

    // Shut down the workers.
    let mut joins = tokio::task::JoinSet::new();
    for w in workers {
        joins.spawn(async move {
            let _ = w.await;
        });
    }
    while joins.join_next().await.is_some() {}

    // Shut down any remaining nodes.
    let remaining: Vec<_> = pool.lock().drain(..).collect();
    let mut joins = tokio::task::JoinSet::new();
    for node in remaining {
        let s_stats = stats.clone();
        joins.spawn(async move {
            node.node().shut_down().await;
            s_stats.nodes_shutdown.fetch_add(1, Ordering::Relaxed);
        });
    }
    while joins.join_next().await.is_some() {}

    // Print final metrics.
    let snap = Snapshot::capture(&stats);
    print_metrics(start, 0, &snap, &snap);

    // Show heap stats.
    // println!("\nheap stats:\n{}", GLOBAL.stats());

    // Check some invariants.
    assert_eq!(snap.nodes_spawned, snap.nodes_shutdown);
    assert_eq!(snap.on_connect_fired, snap.on_disconnect_fired);
    assert_eq!(snap.in_flight_connects, 0);
    assert_eq!(snap.in_flight_disconnects, 0);
    assert_eq!(snap.in_flight_spawns, 0);
    assert_eq!(snap.in_flight_shutdowns, 0);

    println!("\nAll the invariants held");
}
