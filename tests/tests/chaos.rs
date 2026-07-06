//! Extreme random churn of nodes, connections, and messages, designed to run indefinitely.
//!
//! Multiple worker tasks roll dice and execute random actions against a
//! shared pool of nodes. They race against each other and against
//! in-progress shutdowns. The metrics printer reports totals and per-second
//! rates every few seconds.
//!
//! Run with:
//!     cargo test -p tests --profile chaos chaos -- --ignored --nocapture
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
//! The action mix is swarm-tested: every `CHAOS_EPOCH_SECS` (default 30) the
//! weights are re-rolled from the seed - some actions dominate, others are
//! omitted entirely - so successive epochs explore different regimes instead
//! of a single hand-tuned mix. The epoch weight *sequence* is deterministic
//! given the seed; epoch boundaries are wall-clock, so which actions land in
//! which epoch is not. Set `CHAOS_SWARM=0` to pin the classic static mix.
//!
//! Pacing is adaptive: a governor task measures executor lag (timer overshoot)
//! and steers the per-action delay ceiling so the runtime stays in a
//! contended-but-alive regime whatever the host's capacity - by design this
//! part is machine-dependent. Set `CHAOS_GOVERNOR=0` to pin the static delay
//! range instead.
//!
//! Every so often the test runs a burst: a swarm of short-lived extra
//! workers floods the pool with zero-delay actions, deliberately pushing the
//! executor into the lagging, overloaded regime the governor otherwise
//! steers away from - and then releases the pressure, exercising recovery.
//! The governor pauses its steering while a burst is active. Set
//! `CHAOS_BURST=0` to disable bursts.
//!
//! Alongside the end-of-run invariants, watchdogs run *during* the test:
//! no action may exceed a generous age bound (that's a wedge, not
//! congestion), the workers as a whole must keep completing actions,
//! sampled nodes must respect their configured connection limits,
//! `shut_down` must leave no active connections behind, and the
//! file-descriptor and task counts must stay under generous ceilings
//! (that's a leak, not a spike). A violation fails the run immediately.
//! Set `CHAOS_WATCHDOG=0` to disable them.
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
//! On resource-constrained CI runners the governor throttles pacing on its
//! own; reducing MAX_NODES and NUM_WORKERS additionally lowers the memory and
//! file-descriptor footprint. The action-mix coverage is preserved either
//! way, just at proportionally lower throughput.

use std::{
    // alloc::System,
    env,
    fmt,
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::{Bytes, BytesMut};
// use heapster::Heapster;
use parking_lot::{Mutex, RwLock};
use pea2pea::{
    Config, Connection, ConnectionSide, Node, Pea2Pea,
    connections::DisconnectOrigin,
    protocols::{Handshake, OnConnect, OnDisconnect, Reading, Writing},
};
use rand::{RngExt, SeedableRng, prelude::IndexedRandom, rngs::SmallRng};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpSocket,
    sync::mpsc,
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
/// enough to let other workers interleave between actions. With the governor
/// enabled the ceiling is only the starting point.
const MIN_ACTION_DELAY_US: u64 = 0;
const MAX_ACTION_DELAY_US: u64 = 500;
/// Message size bounds.
const MIN_MSG_SIZE: usize = 1;
const MAX_MSG_SIZE: usize = 4096;

/// How often the swarm sampler re-rolls the action mix.
const DEFAULT_EPOCH_SECS: u64 = 30;

// Governor bounds and targets. Executor lag is measured as timer overshoot
// (EWMA, in microseconds); the delay ceiling is steered multiplicatively
// between the floor and the cap to keep the lag inside the target band.
const LAG_PROBE: Duration = Duration::from_millis(5);
/// The governor adjusts the delay ceiling once per this many lag probes.
const GOVERNOR_STEER_EVERY: u32 = 20;
const LAG_HIGH_US: f64 = 8_000.0;
const LAG_LOW_US: f64 = 2_000.0;
const DELAY_FLOOR_US: u64 = 100;
const DELAY_CAP_US: u64 = 100_000;

// Burst bounds. A burst adds `NUM_WORKERS..=BURST_WORKERS_MAX` extra
// workers and suspends all action delays for its duration; the gaps between
// bursts leave enough room to observe a full recovery.
const BURST_WORKERS_MAX: usize = 64;
const BURST_MIN_MS: u64 = 1_000;
const BURST_MAX_MS: u64 = 5_000;
const BURST_GAP_MIN_MS: u64 = 10_000;
const BURST_GAP_MAX_MS: u64 = 40_000;

// Watchdog bounds. Every action is designed to conclude within seconds (the
// connect path is the slowest: the TCP connect timeout plus the handshake
// timeout), so an action older than the limit is a wedge, not congestion.
const ACTION_AGE_LIMIT: Duration = Duration::from_secs(30);
/// Consecutive all-idle metrics windows tolerated before declaring a stall.
const STALL_WINDOW_LIMIT: u32 = 3;
// Resource ceilings, set at a generous multiple of the theoretical peak
// (nodes x connections x descriptors/tasks per connection, plus the handler
// tasks and in-flight setups); an overshoot means a leak, not a spike.
const FD_LIMIT: usize = 4096;
const TASK_LIMIT: usize = 10_000;

// Modest spread is enough - 64 sources splits the hash contention 64-way.
// 127.0.0.2 .. 127.0.0.65 (skip .0 and .1, commonly used).
const SRC_IP_COUNT: u32 = 64;
static SRC_IP_CURSOR: AtomicU32 = AtomicU32::new(0);

static MSG_BYTES: &[u8] = &[0xAB; MAX_MSG_SIZE];

// =========================================================================
// Action mix
// =========================================================================

#[derive(Clone, Copy)]
enum Action {
    Spawn,
    Shutdown,
    ToggleListener,
    Connect,
    Disconnect,
    Broadcast,
    Unicast,
}

/// The actions and their names, in `Action` discriminant order (relied
/// upon by the watchdog's slot tags).
const ACTIONS: [(Action, &str); 7] = [
    (Action::Spawn, "spawn"),
    (Action::Shutdown, "shutdown"),
    (Action::ToggleListener, "toggle_listener"),
    (Action::Connect, "connect"),
    (Action::Disconnect, "disconnect"),
    (Action::Broadcast, "broadcast"),
    (Action::Unicast, "unicast"),
];

/// Action weights in parts of 10_000, in `ACTIONS` order.
#[derive(Clone, Copy)]
struct Weights([u16; 7]);

impl Weights {
    /// The original hand-tuned mix, compiled with tokio's survival in mind -
    /// too many spawns and shutdowns bog down the executor and the OS.
    fn classic() -> Self {
        Self([10, 2, 30, 3988, 3988, 1012, 970])
    }

    /// Swarm-style sampling. The structural actions get bounded random
    /// weights (they are disproportionately expensive per action - tasks,
    /// sockets, OS state), while the traffic actions split the remaining
    /// mass in random proportions, each omitted entirely with 25%
    /// probability so that some epochs explore regimes a balanced mix
    /// never reaches.
    fn swarm(rng: &mut SmallRng) -> Self {
        // a small spawn floor, so that an emptied pool always recovers
        let spawn = rng.random_range(5..=300u32);
        let shutdown = rng.random_range(0..=300u32);
        let toggle = rng.random_range(0..=500u32);

        let mut shares = [0u32; 4];
        while shares.iter().all(|s| *s == 0) {
            for share in &mut shares {
                *share = if rng.random_range(0..4u8) == 0 {
                    0
                } else {
                    rng.random_range(1..=1_000)
                };
            }
        }
        let total: u32 = shares.iter().sum();
        let remaining = 10_000 - spawn - shutdown - toggle;

        let mut weights = [spawn as u16, shutdown as u16, toggle as u16, 0, 0, 0, 0];
        let mut assigned = 0;
        for (weight, share) in weights[3..].iter_mut().zip(shares) {
            *weight = (share as u64 * remaining as u64 / total as u64) as u16;
            assigned += *weight as u32;
        }
        // rounding leftovers go to the largest traffic share
        let largest = shares
            .iter()
            .enumerate()
            .max_by_key(|(_, share)| **share)
            .unwrap()
            .0;
        weights[3 + largest] += (remaining - assigned) as u16;

        Self(weights)
    }

    fn pick(&self, roll: u16) -> Action {
        let mut acc = 0u16;
        for ((action, _), weight) in ACTIONS.iter().zip(self.0) {
            acc += weight;
            if roll < acc {
                return *action;
            }
        }
        Action::Unicast
    }
}

impl fmt::Display for Weights {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const NAMES: [&str; 7] = ["spawn", "shut", "toggle", "conn", "disc", "bcast", "ucast"];
        for (i, (name, weight)) in NAMES.iter().zip(self.0).enumerate() {
            if i > 0 {
                write!(f, " ")?;
            }
            write!(f, "{name}={:.2}%", weight as f64 / 100.0)?;
        }
        Ok(())
    }
}

/// Runtime-adjustable dials and shared knobs: the swarm sampler rotates the
/// action mix, the governor steers the delay ceiling (and reports the lag it
/// measures), and the workers read them on every action.
struct Dials {
    weights: RwLock<Weights>,
    max_delay_us: AtomicU64,
    sched_lag_us: AtomicU64,
    /// Set while an external actor (the burst director) holds the delay
    /// ceiling; the governor doesn't steer while it's up.
    steering_paused: AtomicBool,
    fast_timeouts: bool,
}

// =========================================================================
// Watchdogs
// =========================================================================

/// What a worker is currently doing: `action` is 0 when idle, otherwise the
/// action's index in `ACTIONS` plus one; `started_us` is the action's start
/// time in microseconds since the test began. Padded to a cache line so the
/// per-iteration stores of neighboring workers don't false-share.
#[derive(Default)]
#[repr(align(64))]
struct ActionSlot {
    action: AtomicU8,
    started_us: AtomicU64,
    /// The worker's loop-iteration count, sharded here to avoid a single
    /// contended counter; the stall watchdog sums the slots.
    rolls: AtomicUsize,
}

/// Shared watchdog state: per-worker action slots (the age and stall
/// checks) and the channel that reports violations to the main task, which
/// fails the test upon receipt.
struct Watch {
    enabled: bool,
    start: Instant,
    /// One slot per regular worker plus one per potential burst worker
    /// (burst workers index with `NUM_WORKERS + idx`).
    slots: [ActionSlot; NUM_WORKERS + BURST_WORKERS_MAX],
    violations: mpsc::UnboundedSender<String>,
}

impl Watch {
    fn report(&self, msg: String) {
        if self.enabled {
            let _ = self.violations.send(msg);
        }
    }
}

/// The watchdog driver's iteration state, owned by the metrics task.
#[derive(Default)]
struct WatchdogCursor {
    tick: usize,
    prev_rolls: usize,
    idle_windows: u32,
}

impl WatchdogCursor {
    /// The runtime checks, invoked once per metrics interval.
    async fn check(&mut self, watch: &Watch, pool: &Pool) {
        if !watch.enabled {
            return;
        }
        self.tick += 1;

        // No action may outlive its designed bounds.
        let now_us = watch.start.elapsed().as_micros() as u64;
        let limit_us = ACTION_AGE_LIMIT.as_micros() as u64;
        for (idx, slot) in watch.slots.iter().enumerate() {
            let tag = slot.action.load(Ordering::Acquire);
            if tag == 0 {
                continue;
            }
            let age_us = now_us.saturating_sub(slot.started_us.load(Ordering::Relaxed));
            if age_us > limit_us {
                let name = ACTIONS[(tag - 1) as usize].1;
                watch.report(format!(
                    "worker {idx} has been stuck in '{name}' for {:.1}s",
                    age_us as f64 / 1_000_000.0
                ));
            }
        }

        // The workers as a whole must keep completing actions; the count only
        // stops advancing if every single worker is wedged (or the executor is).
        let rolls: usize = watch
            .slots
            .iter()
            .map(|slot| slot.rolls.load(Ordering::Relaxed))
            .sum();
        if rolls == self.prev_rolls {
            self.idle_windows += 1;
            if self.idle_windows >= STALL_WINDOW_LIMIT {
                watch.report(format!(
                    "the workers haven't started a single action across \
                     {STALL_WINDOW_LIMIT} consecutive metrics windows"
                ));
            }
        } else {
            self.idle_windows = 0;
        }
        self.prev_rolls = rolls;

        // Sampled per-node limit checks. These are hard invariants, but the two
        // reads aren't atomic with respect to each other, so re-check after a
        // beat before declaring a violation.
        let sampled = {
            let p = pool.lock();
            if p.is_empty() {
                None
            } else {
                Some(p[self.tick % p.len()].clone())
            }
        };
        if let Some(n) = sampled {
            let limits_exceeded = |n: &StressNode| {
                n.node().num_connected() > MAX_NODES || n.node().num_connecting() > MAX_NODES / 2
            };
            if limits_exceeded(&n) {
                sleep(Duration::from_millis(250)).await;
                if limits_exceeded(&n) {
                    watch.report(format!(
                        "a node exceeds its connection limits: connected={}/{MAX_NODES}, \
                         connecting={}/{}",
                        n.node().num_connected(),
                        n.node().num_connecting(),
                        MAX_NODES / 2,
                    ));
                }
            }
        }

        // Resource ceilings.
        if let Some(fds) = fd_count()
            && fds > FD_LIMIT
        {
            watch.report(format!(
                "the file descriptor count exceeded its ceiling: {fds} > {FD_LIMIT}"
            ));
        }
        let tasks = tokio::runtime::Handle::current()
            .metrics()
            .num_alive_tasks();
        if tasks > TASK_LIMIT {
            watch.report(format!(
                "the alive task count exceeded its ceiling: {tasks} > {TASK_LIMIT}"
            ));
        }
    }
}

/// The number of open file descriptors (`None` where /proc is unavailable).
fn fd_count() -> Option<usize> {
    std::fs::read_dir("/proc/self/fd").ok().map(|d| d.count())
}

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
    listener_toggles: AtomicUsize,
    fast_send: AtomicUsize,
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

#[derive(Default)]
struct Snapshot {
    nodes_spawned: usize,
    nodes_shutdown: usize,
    connects_succeeded: usize,
    disconnects: usize,
    listener_toggles: usize,
    fast_send: usize,
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
            listener_toggles: s.listener_toggles.load(Ordering::Relaxed),
            fast_send: s.fast_send.load(Ordering::Relaxed),
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
        // enable all the protocols before the listener goes live: an inbound
        // connection accepted mid-install would otherwise skip the hooks that
        // aren't armed yet and skew the on_connect/on_disconnect accounting
        // (reachable via a recycled listener port, under heavy churn)
        tokio::join!(
            self.enable_handshake(),
            self.enable_reading(),
            self.enable_writing(),
            self.enable_on_connect(),
            self.enable_on_disconnect(),
        );
        self.node().toggle_listener().await?;
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

fn msg_codec() -> LengthDelimitedCodec {
    LengthDelimitedCodec::builder()
        .max_frame_length(MAX_MSG_SIZE)
        .new_codec()
}

impl Reading for StressNode {
    type Message = BytesMut;
    type Codec = LengthDelimitedCodec;

    const INITIAL_BUFFER_SIZE: usize = 4 * 1024;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        msg_codec()
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
        msg_codec()
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
    pool.lock().choose(rng).cloned()
}

fn pick_two(pool: &Pool, rng: &mut SmallRng) -> Option<(StressNode, StressNode)> {
    let p = pool.lock();
    if p.len() < 2 {
        return None;
    }
    let sampled = p.sample_array::<_, 2>(rng)?;
    Some((sampled[0].clone(), sampled[1].clone()))
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
    let pushed = if node.install().await.is_ok() {
        let mut pool = pool.lock();
        if !token.is_cancelled() && pool.len() < MAX_NODES {
            pool.push(node.clone());
            true
        } else {
            false
        }
    } else {
        false
    };
    if pushed {
        stats.nodes_spawned.fetch_add(1, Ordering::Relaxed);
    } else {
        node.node().shut_down().await;
    }
}

async fn act_shutdown(pool: &Pool, stats: &Arc<Stats>, watch: &Watch, rng: &mut SmallRng) {
    if let Some(node) = pop_random(pool, rng) {
        let _guard = InFlightGuard::new(&stats.in_flight_shutdowns);
        node.node().shut_down().await;
        stats.nodes_shutdown.fetch_add(1, Ordering::Relaxed);
        // the cleanup contract: once shut_down returns, the active set is
        // drained (in-flight setups may still hold `connecting` entries, so
        // only the active count is checked here)
        let leftover = node.node().num_connected();
        if leftover != 0 {
            watch.report(format!(
                "a node still had {leftover} active connection(s) after shut_down"
            ));
        }
        // dropping `node` here releases the local Arc clone; if no worker is
        // mid-action on it, the InnerNode Arc count goes to zero shortly
    }
}

async fn act_toggle_listener(pool: &Pool, stats: &Arc<Stats>, rng: &mut SmallRng) {
    let Some(a) = pick_one(pool, rng) else { return };
    // flap: off, briefly dark, back on; either call may legitimately fail
    // if a shutdown races in - that race is precisely the coverage we want
    if matches!(a.node().toggle_listener().await, Ok(None)) {
        sleep(Duration::from_micros(rng.random_range(0..2_000))).await;
        let _ = a.node().toggle_listener().await;
        stats.listener_toggles.fetch_add(1, Ordering::Relaxed);
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
    let target = *peers.choose(rng).unwrap();
    let _guard = InFlightGuard::new(&stats.in_flight_disconnects);
    if a.node().disconnect(target).await {
        stats.disconnects.fetch_add(1, Ordering::Relaxed);
    }
}

async fn act_broadcast(pool: &Pool, stats: &Arc<Stats>, rng: &mut SmallRng) {
    let Some(a) = pick_one(pool, rng) else { return };
    let peers = a.node().connected_addrs();
    if peers.is_empty() {
        return;
    }
    let msg = random_message(rng);
    for addr in peers {
        match a.unicast_fast(addr, msg.clone()) {
            Ok(_) => {
                stats.fast_send.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                stats.err_send.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

async fn act_unicast(pool: &Pool, stats: &Arc<Stats>, rng: &mut SmallRng) {
    let Some(a) = pick_one(pool, rng) else { return };
    let peers = a.node().connected_addrs();
    if peers.is_empty() {
        return;
    }
    let target = *peers.choose(rng).unwrap();
    let msg = random_message(rng);
    stats.unicasts_attempted.fetch_add(1, Ordering::Relaxed);
    match a.unicast(target, msg) {
        Ok(rx) => {
            match rx.await {
                Ok(Ok(())) => stats.unicasts_succeeded.fetch_add(1, Ordering::Relaxed),
                _ => stats.err_send.fetch_add(1, Ordering::Relaxed),
            };
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
    dials: Arc<Dials>,
    watch: Arc<Watch>,
    idx: usize,
    token: CancellationToken,
    mut rng: SmallRng,
) {
    let slot = &watch.slots[idx];
    while !token.is_cancelled() {
        let roll: u16 = rng.random_range(..10_000);
        // the read guard must be dropped before the action is awaited
        let action = dials.weights.read().pick(roll);
        if watch.enabled {
            slot.rolls.fetch_add(1, Ordering::Relaxed);
            slot.started_us
                .store(watch.start.elapsed().as_micros() as u64, Ordering::Relaxed);
            slot.action.store(action as u8 + 1, Ordering::Release);
        }
        match action {
            Action::Spawn => act_spawn(&pool, &stats, &token).await,
            Action::Shutdown => act_shutdown(&pool, &stats, &watch, &mut rng).await,
            Action::ToggleListener => act_toggle_listener(&pool, &stats, &mut rng).await,
            Action::Connect => {
                act_connect(&pool, &stats, &mut rng, &token, dials.fast_timeouts).await
            }
            Action::Disconnect => act_disconnect(&pool, &stats, &mut rng).await,
            Action::Broadcast => act_broadcast(&pool, &stats, &mut rng).await,
            Action::Unicast => act_unicast(&pool, &stats, &mut rng).await,
        }
        if watch.enabled {
            slot.action.store(0, Ordering::Release);
        }

        if token.is_cancelled() {
            break;
        }

        // during a burst the dial is held at zero, which makes this a no-op;
        // cancellation is picked up at the top of the next iteration
        let max_delay_us = dials.max_delay_us.load(Ordering::Relaxed);
        let sleep_us = rng.random_range(MIN_ACTION_DELAY_US..=max_delay_us);
        if sleep_us > 0 {
            sleep(Duration::from_micros(sleep_us)).await;
        }
    }
}

// =========================================================================
// Governor
// =========================================================================

/// Measures executor lag as timer overshoot and steers the delay ceiling to
/// keep the lag inside the target band: multiplicative increase for a fast
/// retreat when the runtime bogs down, gentle decrease to creep back toward
/// full pressure once it recovers.
async fn governor(dials: Arc<Dials>, token: CancellationToken) {
    let mut ewma_us = 0.0f64;
    let mut probes = 0u32;

    while !token.is_cancelled() {
        let start = Instant::now();
        tokio::select! {
            _ = sleep(LAG_PROBE) => {}
            _ = token.cancelled() => break,
        }
        let overshoot_us = start.elapsed().saturating_sub(LAG_PROBE).as_micros() as f64;
        ewma_us = ewma_us * 0.9 + overshoot_us * 0.1;
        dials.sched_lag_us.store(ewma_us as u64, Ordering::Relaxed);

        probes += 1;
        // during a burst the lag is expected - keep measuring, don't steer
        if probes.is_multiple_of(GOVERNOR_STEER_EVERY)
            && !dials.steering_paused.load(Ordering::Relaxed)
        {
            let cur = dials.max_delay_us.load(Ordering::Relaxed);
            let next = if ewma_us > LAG_HIGH_US {
                (cur * 2).min(DELAY_CAP_US)
            } else if ewma_us < LAG_LOW_US {
                (cur * 7 / 8).max(DELAY_FLOOR_US)
            } else {
                cur
            };
            dials.max_delay_us.store(next, Ordering::Relaxed);
        }
    }
}

// =========================================================================
// Epochs
// =========================================================================

/// Re-rolls the action mix once per epoch.
async fn epoch_rotator(
    dials: Arc<Dials>,
    token: CancellationToken,
    mut rng: SmallRng,
    epoch_len: Duration,
) {
    let mut epoch = 0u64;
    loop {
        tokio::select! {
            _ = sleep(epoch_len) => {
                epoch += 1;
                let weights = Weights::swarm(&mut rng);
                *dials.weights.write() = weights;
                println!("[epoch {epoch}] mix: {weights}");
            }
            _ = token.cancelled() => break,
        }
    }
}

// =========================================================================
// Bursts
// =========================================================================

/// Periodically floods the pool with short-lived extra workers running at
/// zero delay; the executor is expected to lag during a storm - the point
/// is to exercise overload and, crucially, clean recovery from it.
async fn burst_director(
    pool: Pool,
    stats: Arc<Stats>,
    dials: Arc<Dials>,
    watch: Arc<Watch>,
    token: CancellationToken,
    seed: u64,
) {
    let mut rng = SmallRng::seed_from_u64(seed);
    loop {
        let gap = rng.random_range(BURST_GAP_MIN_MS..=BURST_GAP_MAX_MS);
        tokio::select! {
            _ = sleep(Duration::from_millis(gap)) => {}
            _ = token.cancelled() => break,
        }

        let extra = rng.random_range(NUM_WORKERS..=BURST_WORKERS_MAX);
        let len = rng.random_range(BURST_MIN_MS..=BURST_MAX_MS);
        println!(
            "{} burst: +{extra} workers for {:.1}s",
            stamp(watch.start),
            len as f64 / 1_000.0,
        );
        // burst pressure flows through the regular delay dial: hold it at
        // zero for the duration, with the governor's steering paused
        dials.steering_paused.store(true, Ordering::Relaxed);
        let saved_delay = dials.max_delay_us.swap(0, Ordering::Relaxed);

        let storm_token = token.child_token();
        let mut storm = tokio::task::JoinSet::new();
        for idx in 0..extra {
            let worker_seed: u64 = rng.random();
            storm.spawn(worker(
                pool.clone(),
                stats.clone(),
                dials.clone(),
                watch.clone(),
                NUM_WORKERS + idx,
                storm_token.clone(),
                SmallRng::seed_from_u64(worker_seed),
            ));
        }
        tokio::select! {
            _ = sleep(Duration::from_millis(len)) => {}
            _ = token.cancelled() => {}
        }
        storm_token.cancel();
        while storm.join_next().await.is_some() {}

        dials.max_delay_us.store(saved_delay, Ordering::Relaxed);
        dials.steering_paused.store(false, Ordering::Relaxed);
        println!("{} burst over", stamp(watch.start));
    }
}

// =========================================================================
// Metrics
// =========================================================================

/// The `[t=...]` prefix shared by all timestamped log lines.
fn stamp(start: Instant) -> String {
    format!("[t={:>5.0}s]", start.elapsed().as_secs_f64())
}

fn print_metrics(start: Instant, alive: usize, dials: &Dials, cur: &Snapshot, prev: &Snapshot) {
    let dt = METRICS_INTERVAL.as_secs_f64();
    let rate = |c: usize, p: usize| (c.saturating_sub(p)) as f64 / dt;

    println!(
        "{stamp} alive={alive:>2}/{max} | \
         nodes spawned/shut={ns}/{nd} | \
         listener toggles={lt} | \
         conn att/ok/err={ca}/{cs}/{ce} | disc={dc} | \
         ufast={fs} ucast att/ok={ua}/{us} send-err={se} | \
         recv={rv} | on_c/on_d={oc}/{od} | \
         ifc={ifc} | ifdc={ifdc} | ifsp={ifsp} | ifsd={ifsd}",
        stamp = stamp(start),
        max = MAX_NODES,
        ns = cur.nodes_spawned,
        nd = cur.nodes_shutdown,
        lt = cur.listener_toggles,
        ca = cur.connects_succeeded + cur.err_connect,
        cs = cur.connects_succeeded,
        ce = cur.err_connect,
        dc = cur.disconnects,
        fs = cur.fast_send,
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
        "           Δ/s: conn={:.1} disc={:.1} ufast={:.1} ucast={:.1} recv={:.1} list={:.1} \
         | lag={:.1}ms delay_cap={}us",
        rate(cur.connects_succeeded, prev.connects_succeeded),
        rate(cur.disconnects, prev.disconnects),
        rate(cur.fast_send, prev.fast_send),
        rate(cur.unicasts_succeeded, prev.unicasts_succeeded),
        rate(cur.msgs_received, prev.msgs_received),
        rate(cur.listener_toggles, prev.listener_toggles),
        dials.sched_lag_us.load(Ordering::Relaxed) as f64 / 1_000.0,
        dials.max_delay_us.load(Ordering::Relaxed),
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

/// Reads and parses an env var; `None` when unset or unparsable.
fn env_parse<T: std::str::FromStr>(name: &str) -> Option<T> {
    env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

/// Reads a boolean env var ("0"/"false"/"no" disable, anything else enables).
fn env_flag(name: &str, default: bool) -> bool {
    env::var(name)
        .map(|v| !matches!(v.trim(), "0" | "false" | "no"))
        .unwrap_or(default)
}

async fn infinite_chaos_inner() {
    // Determine and log the master seed. CHAOS_SEED overrides for reruns.
    let master_seed: u64 = env_parse("CHAOS_SEED").unwrap_or_else(rand::random);
    let runtime = env_parse("CHAOS_RUNTIME_SECS")
        .map(Duration::from_secs)
        .unwrap_or(Duration::MAX);
    let fast_timeouts = env_flag("CHAOS_FAST_TIMEOUTS", false);
    let swarm_enabled = env_flag("CHAOS_SWARM", true);
    let governor_enabled = env_flag("CHAOS_GOVERNOR", true);
    let watchdogs_enabled = env_flag("CHAOS_WATCHDOG", true);
    let bursts_enabled = env_flag("CHAOS_BURST", true);
    let epoch_len =
        Duration::from_secs(env_parse("CHAOS_EPOCH_SECS").unwrap_or(DEFAULT_EPOCH_SECS));

    println!(
        "pea2pea stress test - {NUM_WORKERS} workers, up to {MAX_NODES} nodes\n\
         seed: {master_seed}\n\
         swarm mix: {} | governor: {} | watchdogs: {} | bursts: {}",
        if swarm_enabled {
            format!("on, epoch {}s", epoch_len.as_secs())
        } else {
            "off (classic mix)".into()
        },
        if governor_enabled { "on" } else { "off" },
        if watchdogs_enabled { "on" } else { "off" },
        if bursts_enabled { "on" } else { "off" },
    );

    let mut master_rng = SmallRng::seed_from_u64(master_seed);
    let stats: Arc<Stats> = Default::default();
    let pool: Pool = Default::default();
    let token = CancellationToken::new();

    // The swarm RNG is derived from the master seed up front, so the epoch
    // weight sequence is reproducible regardless of anything that follows.
    let mut swarm_rng = SmallRng::seed_from_u64(master_rng.random());
    let burst_seed: u64 = master_rng.random();
    let initial_weights = if swarm_enabled {
        Weights::swarm(&mut swarm_rng)
    } else {
        Weights::classic()
    };
    println!("[epoch 0] mix: {initial_weights}\n");
    let dials = Arc::new(Dials {
        weights: RwLock::new(initial_weights),
        max_delay_us: AtomicU64::new(MAX_ACTION_DELAY_US),
        sched_lag_us: AtomicU64::new(0),
        steering_paused: AtomicBool::new(false),
        fast_timeouts,
    });

    // Rotate the action mix every epoch.
    if swarm_enabled {
        tokio::spawn(epoch_rotator(
            dials.clone(),
            token.clone(),
            swarm_rng,
            epoch_len,
        ));
    }

    // Keep the pressure adaptive.
    if governor_enabled {
        tokio::spawn(governor(dials.clone(), token.clone()));
    }

    // Seed the pool with two nodes so workers have something to do immediately.
    for _ in 0..2 {
        let n = StressNode::new(stats.clone());
        n.install().await.unwrap();
        stats.nodes_spawned.fetch_add(1, Ordering::Relaxed);
        pool.lock().push(n);
    }

    // Set up the watchdog state and the violation channel; a violation
    // failing the main select below is what fails the test.
    let start = Instant::now();
    let (viol_tx, mut viol_rx) = mpsc::unbounded_channel();
    let watch = Arc::new(Watch {
        enabled: watchdogs_enabled,
        start,
        slots: std::array::from_fn(|_| ActionSlot::default()),
        violations: viol_tx,
    });

    // Spawn workers, each with a derived RNG seeded from the master.
    let mut workers = Vec::with_capacity(NUM_WORKERS);
    for idx in 0..NUM_WORKERS {
        let worker_seed: u64 = master_rng.random();
        let worker_rng = SmallRng::seed_from_u64(worker_seed);
        workers.push(tokio::spawn(worker(
            pool.clone(),
            stats.clone(),
            dials.clone(),
            watch.clone(),
            idx,
            token.clone(),
            worker_rng,
        )));
    }

    // Periodically flood the pool with short-lived extra workers.
    if bursts_enabled {
        tokio::spawn(burst_director(
            pool.clone(),
            stats.clone(),
            dials.clone(),
            watch.clone(),
            token.clone(),
            burst_seed,
        ));
    }

    // Spawn the metrics printer, which doubles as the watchdog driver.
    let m_stats = stats.clone();
    let m_pool = pool.clone();
    let m_dials = dials.clone();
    let m_watch = watch.clone();
    let m_token = token.clone();
    tokio::spawn(async move {
        let mut prev = Snapshot::default();
        let mut cursor = WatchdogCursor::default();
        loop {
            tokio::select! {
                _ = sleep(METRICS_INTERVAL) => {
                    let snap = Snapshot::capture(&m_stats);
                    let alive = m_pool.lock().len();
                    print_metrics(m_watch.start, alive, &m_dials, &snap, &prev);
                    prev = snap;
                    cursor.check(&m_watch, &m_pool).await;
                }
                _ = m_token.cancelled() => break,
            }
        }
    });

    // Wait for the deadline to expire, a watchdog violation, or Ctrl-C.
    tokio::select! {
        _ = sleep(runtime) => {}
        _ = tokio::signal::ctrl_c() => {}
        viol = viol_rx.recv() => {
            panic!(
                "watchdog violation: {}",
                viol.unwrap_or_else(|| "violation channel closed".into())
            );
        }
    }
    token.cancel();

    println!("\nShutting down...\n");

    // Shut down the workers; they were all cancelled by the token, so a
    // sequential await amounts to a concurrent join.
    for w in workers {
        let _ = w.await;
    }

    // Shut down any remaining nodes; with the workers quiesced, the drain
    // must be total.
    let remaining: Vec<_> = pool.lock().drain(..).collect();
    let mut joins = tokio::task::JoinSet::new();
    for node in remaining {
        let s_stats = stats.clone();
        joins.spawn(async move {
            node.node().shut_down().await;
            s_stats.nodes_shutdown.fetch_add(1, Ordering::Relaxed);
            assert_eq!(
                node.node().num_connected(),
                0,
                "active connections left after the final shut_down"
            );
            assert_eq!(
                node.node().num_connecting(),
                0,
                "pending connections left after the final shut_down"
            );
        });
    }
    joins.join_all().await;

    // Allow a bit of time for the detached OnConnect work to conclude.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let oc = stats.on_connect_fired.load(Ordering::Relaxed);
        let od = stats.on_disconnect_fired.load(Ordering::Relaxed);
        if oc == od {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "OnConnect/OnDisconnect hook counters never converged: {oc} vs {od}"
        );
        sleep(Duration::from_millis(50)).await;
    }

    // Print final metrics.
    let snap = Snapshot::capture(&stats);
    print_metrics(watch.start, 0, &dials, &snap, &snap);

    // Show heap stats.
    // println!("\nheap stats:\n{}", GLOBAL.stats());

    // Check some invariants.
    assert_eq!(snap.nodes_spawned, snap.nodes_shutdown);
    assert_eq!(snap.on_connect_fired, snap.on_disconnect_fired);
    assert_eq!(snap.in_flight_connects, 0);
    assert_eq!(snap.in_flight_disconnects, 0);
    assert_eq!(snap.in_flight_spawns, 0);
    assert_eq!(snap.in_flight_shutdowns, 0);

    // A watchdog violation that raced the shutdown still fails the test.
    if let Ok(viol) = viol_rx.try_recv() {
        panic!("watchdog violation: {viol}");
    }

    println!("\nAll the invariants held");
}
