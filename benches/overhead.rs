//! Library overhead vs. a raw `tokio` + `Framed` baseline.
//!
//! The question: how much does pea2pea cost on top of hand-wiring the same
//! codec onto a tokio socket? The whole comparison only means anything if the
//! baseline does the *same work*, so the design is built around that:
//!
//! * **Identical framing.** pea2pea's codecs are just `tokio_util` Decoder/
//!   Encoder impls, so the baseline uses the very same `TestCodec`,
//!   wrapped in `FramedRead`/`FramedWrite` over a plain `TcpStream`. Both sides
//!   encode and decode byte-for-byte identical frames.
//! * **Identical per-message work.** Both receivers count one frame at a time
//!   (pea2pea via `process_message`, the baseline via `framed.next()`), so
//!   neither gets the unfair advantage of bulk byte counting.
//! * **Identical socket options.** Both sides set `TCP_NODELAY` on every socket
//! * **Identical workload.** Both send `MESSAGES` frames of a given size and
//!   wait for all of them to land before the clock stops, then report
//!   messages/sec via an `ItemsCount` counter.
//!
//! So the measured delta is purely pea2pea's orchestration layer - connection
//! management, addressing, and its reader/writer-task + channel model - over an
//! otherwise identical framing path. (The handshake runs once at connection
//! setup, which is hoisted out of the timed region, so it costs nothing in the
//! steady-state measurement beyond the socket option it sets.)
//!
//! COMPLETION SIGNAL: each receiver fires a `Notify` the moment it has counted a
//! full sample's worth of frames, and the timed closure awaits that, rather than
//! busy-spinning on the counter.
//!
//! BASELINE BEHAVIOUR (documented so the comparison is auditable, not trust-me):
//! the raw server is a single task looping `framed.next()` and counting; the
//! raw client `feed`s every frame and does one final `flush`, relying on
//! `FramedWrite`'s backpressure boundary to coalesce writes to the socket. That
//! coalescing is the fair analog of pea2pea draining its writer channel. The one
//! asymmetry to keep in mind: the baseline is a single read task plus a buffered
//! write sink, whereas pea2pea splits read and write across tasks with a channel
//! between - that architectural cost is part of the overhead, by design.
//!
//! READING IT: the **ratio** pea2pea/raw at a given size is the headline, not
//! the absolute µs - and it travels across hardware far better, since machine
//! noise moves both siblings together. Sweeping message size shows the per-
//! message cost (a fixed tax) amortising: largest relative overhead for tiny
//! high-frequency frames, shrinking as frames grow and I/O dominates.
//!
//! Only steady-state per-message cost is measured here; one connection is held
//! open for the whole run, so connection setup (that's `startup`'s job) and fd
//! limits are not in play.

use std::{
    io,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use bytes::{Bytes, BytesMut};
use divan::{Bencher, counter::ItemsCount};
use futures_util::{SinkExt, StreamExt};
use pea2pea::{
    Connection, ConnectionSide, Node, Pea2Pea,
    protocols::{Handshake, Reading, Writing},
};
use tokio::{
    net::{TcpListener, TcpStream},
    runtime::Runtime,
    sync::Notify,
    time::{sleep, timeout},
};
use tokio_util::codec::{FramedRead, FramedWrite};

#[path = "../tests/common/mod.rs"]
mod common;
use common::{TestCodec, named_node};

fn main() {
    divan::main();
}

/// Frames delivered per timed sample. Tunable - small enough to keep the 4096
/// arg's wall time sane, large enough to dwarf the per-sample fixed costs.
const MESSAGES: usize = 20_000;
const PAYLOAD: u8 = b'x';

/// Message sizes (bytes) to sweep. Tiny frames expose the per-message tax;
/// large frames let it amortise into the I/O cost.
const SIZES: &[usize] = &[16, 256, 4096];

// --- pea2pea side ---

#[derive(Clone)]
struct Receiver {
    node: Node,
    counter: Arc<AtomicUsize>,
    done: Arc<Notify>,
}

impl Pea2Pea for Receiver {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Reading for Receiver {
    type Message = BytesMut;
    type Codec = TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, _source: SocketAddr, _message: Self::Message) {
        // fetch_add is the single crossing point per boundary, so exactly one
        // message per sample trips the signal even under concurrent dispatch
        let count = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        if count % MESSAGES == 0 {
            self.done.notify_one();
        }
    }
}

#[derive(Clone)]
struct Sender {
    node: Node,
}

impl Pea2Pea for Sender {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Writing for Sender {
    type Message = Bytes;
    type Codec = TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

// A no-op handshake whose only purpose is to set TCP_NODELAY on the freshly established stream.
// It exchanges no bytes, so each side completes independently.
macro_rules! nodelay_handshake {
    ($ty:ty) => {
        impl Handshake for $ty {
            async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
                self.borrow_stream(&mut conn).set_nodelay(true)?;
                Ok(conn)
            }
        }
    };
}

nodelay_handshake!(Sender);
nodelay_handshake!(Receiver);

// --- benches ---

/// Raw `tokio` + `Framed` baseline: the floor pea2pea is measured against.
#[divan::bench(sample_count = 50, sample_size = 1, args = SIZES)]
fn raw(bencher: Bencher, size: usize) {
    let rt = runtime();
    let msg = build_msg(size);
    let counter = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(Notify::new());

    // bring up the baselines outside the timed region: a server task counting
    // frames off one accepted connection, and a write sink on the client end;
    // both sockets get TCP_NODELAY to match the pea2pea side.
    let (mut sink, _server) = rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let c = counter.clone();
        let d = done.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            stream.set_nodelay(true).unwrap();
            let mut frames = FramedRead::new(stream, TestCodec::<BytesMut>::default());
            while let Some(frame) = frames.next().await {
                if frame.is_err() {
                    break;
                }
                let count = c.fetch_add(1, Ordering::Relaxed) + 1;
                if count % MESSAGES == 0 {
                    d.notify_one();
                }
            }
        });

        let stream = TcpStream::connect(addr).await.unwrap();
        stream.set_nodelay(true).unwrap();
        let sink = FramedWrite::new(stream, TestCodec::<Bytes>::default());
        sleep(Duration::from_millis(100)).await;
        (sink, server)
    });

    bencher.counter(ItemsCount::new(MESSAGES)).bench_local(|| {
        rt.block_on(async {
            for _ in 0..MESSAGES {
                sink.feed(msg.clone()).await.unwrap();
            }
            sink.flush().await.unwrap();
            await_batch(&done).await;
        });
    });
}

/// The same workload over a pea2pea node pair.
#[divan::bench(sample_count = 50, sample_size = 1, args = SIZES)]
fn pea2pea(bencher: Bencher, size: usize) {
    let rt = runtime();
    let msg = build_msg(size);
    let counter = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(Notify::new());

    let (receiver, sender, addr) = rt.block_on(async {
        let receiver = Receiver {
            node: named_node("recv"),
            counter: counter.clone(),
            done: done.clone(),
        };

        receiver.enable_handshake().await;
        receiver.enable_reading().await;
        let addr = receiver.node().toggle_listener().await.unwrap().unwrap();

        let sender = Sender {
            node: named_node("send"),
        };
        sender.enable_handshake().await;
        sender.enable_writing().await;
        sender.node().connect(addr).await.unwrap();
        sleep(Duration::from_millis(100)).await;
        (receiver, sender, addr)
    });

    bencher.counter(ItemsCount::new(MESSAGES)).bench_local(|| {
        rt.block_on(async {
            for _ in 0..MESSAGES {
                send(|| sender.unicast_fast(addr, msg.clone())).await;
            }
            await_batch(&done).await;
        });
    });

    rt.block_on(async {
        sender.node().shut_down().await;
        receiver.node().shut_down().await;
    });
}

// --- helpers ---

/// One reusable payload of `size` bytes; cloning a `Bytes` is a refcount bump,
/// so per-send cost stays out of the measurement on both sides.
fn build_msg(size: usize) -> Bytes {
    Bytes::from(vec![PAYLOAD; size])
}

/// Retries a `unicast_fast` enqueue, yielding on backpressure (`QuotaExceeded`).
async fn send(mut enqueue: impl FnMut() -> io::Result<()>) {
    loop {
        match enqueue() {
            Ok(_) => break,
            Err(e) if e.kind() == io::ErrorKind::QuotaExceeded => tokio::task::yield_now().await,
            Err(e) => panic!("send failed: {e}"),
        }
    }
}

/// Awaits the receiver's "batch complete" signal. The permit semantics of
/// `notify_one` cover the case where the receiver finishes before we park here,
/// so no wakeup is lost; the deadline only exists so a wiring bug fails loud
/// instead of hanging the whole run.
async fn await_batch(done: &Notify) {
    if timeout(Duration::from_secs(30), done.notified())
        .await
        .is_err()
    {
        panic!("timed out waiting for the batch to be delivered");
    }
}

fn runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}
