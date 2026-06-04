//! Packets-per-second (PPS) throughput.
//!
//! Two send paths:
//! 1. **naive** - one `unicast_fast` per byte. The bottleneck is the per-call
//!    send path, so this is dominated by call overhead.
//! 2. **batch** - one `unicast_fast` expands to `N` bytes via the codec, so the
//!    same payload is delivered in `PACKETS / N` calls. Sweeping `N` shows the
//!    per-call overhead being amortised away.
//!
//! Both confirm receipt before stopping the clock, so both measure *delivered*
//! throughput; the naive-vs-batch gap isolates send-call overhead specifically.

use std::{
    io,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::{Buf, BufMut, BytesMut};
use divan::{Bencher, counter::ItemsCount};
use pea2pea::{
    ConnectionSide, Node, Pea2Pea,
    protocols::{Reading, Writing},
};
use tokio::{runtime::Runtime, time::sleep};
use tokio_util::codec::{Decoder, Encoder};

#[path = "../tests/common/mod.rs"]
mod common;
use common::named_node;

fn main() {
    divan::main();
}

/// Bytes delivered per timed sample. Must stay divisible by every batch size in
/// the `args` sweep below so the batch path sends a whole number of frames.
const PACKETS: usize = 100_000;
const PAYLOAD_BYTE: u8 = b'x';

// --- codecs ---

/// Treats every single byte as a message; this is what the receiver always
/// decodes with, so the receiver-side cost is identical across both send paths.
#[derive(Default)]
struct RawByteCodec;

impl Decoder for RawByteCodec {
    type Item = u8;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.has_remaining() {
            Ok(Some(src.get_u8()))
        } else {
            Ok(None)
        }
    }
}

impl Encoder<u8> for RawByteCodec {
    type Error = io::Error;

    fn encode(&mut self, item: u8, dst: &mut BytesMut) -> Result<(), Self::Error> {
        dst.reserve(1);
        dst.put_u8(item);
        Ok(())
    }
}

/// Expands a single `(byte, count)` tuple into `count` raw bytes in one write.
struct BatchRawCodec;

impl Encoder<(u8, usize)> for BatchRawCodec {
    type Error = io::Error;

    fn encode(
        &mut self,
        (byte, count): (u8, usize),
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        dst.reserve(count);
        dst.put_bytes(byte, count);
        Ok(())
    }
}

// --- nodes ---

/// Reading-only node that counts every byte it decodes.
#[derive(Clone)]
struct Receiver {
    node: Node,
    counter: Arc<AtomicUsize>,
}

impl Pea2Pea for Receiver {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Reading for Receiver {
    type Message = u8;
    type Codec = RawByteCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, _source: SocketAddr, _message: Self::Message) {
        self.counter.fetch_add(1, Ordering::Relaxed);
    }
}

/// Writing-only node that sends one byte per `unicast_fast`.
#[derive(Clone)]
struct NaiveSender {
    node: Node,
}

impl Pea2Pea for NaiveSender {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Writing for NaiveSender {
    type Message = u8;
    type Codec = RawByteCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

/// Writing-only node that sends `(byte, count)`, expanded by the codec.
#[derive(Clone)]
struct BatchSender {
    node: Node,
}

impl Pea2Pea for BatchSender {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Writing for BatchSender {
    type Message = (u8, usize);
    type Codec = BatchRawCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        BatchRawCodec
    }
}

// --- benches ---

/// One `unicast_fast` per byte. Heavy on the per-call send path.
///
/// Node setup, the connection, and teardown all sit outside the timed closure;
/// only the send-and-confirm loop is measured. `sample_size = 1` keeps each
/// sample equal to exactly one `PACKETS` delivery so the counter delta lines up
/// with what divan times.
#[divan::bench(sample_count = 100, sample_size = 1)]
fn naive(bencher: Bencher) {
    let rt = runtime();
    let (receiver, counter, addr) = rt.block_on(spawn_receiver("recv_naive"));

    let sender = NaiveSender {
        node: named_node("send_naive"),
    };
    rt.block_on(connect(&sender, addr));

    bencher.counter(ItemsCount::new(PACKETS)).bench_local(|| {
        rt.block_on(async {
            let target = counter.load(Ordering::Relaxed) + PACKETS;
            for _ in 0..PACKETS {
                send(|| sender.unicast_fast(addr, PAYLOAD_BYTE)).await;
            }
            await_delivery(&counter, target).await;
        });
    });

    rt.block_on(async {
        sender.node().shut_down().await;
        receiver.node().shut_down().await;
    });
}

/// Same delivered payload, but coalesced into `PACKETS / batch_size` writes.
/// The `args` sweep shows per-call overhead being amortised as the batch grows.
#[divan::bench(sample_count = 100, sample_size = 1, args = [10, 100, 1000])]
fn batch(bencher: Bencher, batch_size: usize) {
    assert_eq!(
        PACKETS % batch_size,
        0,
        "PACKETS must divide evenly by batch_size"
    );

    let rt = runtime();
    let (receiver, counter, addr) = rt.block_on(spawn_receiver("recv_batch"));

    let sender = BatchSender {
        node: named_node("send_batch"),
    };
    rt.block_on(connect(&sender, addr));

    let frames = PACKETS / batch_size;

    bencher.counter(ItemsCount::new(PACKETS)).bench_local(|| {
        rt.block_on(async {
            let target = counter.load(Ordering::Relaxed) + PACKETS;
            for _ in 0..frames {
                send(|| sender.unicast_fast(addr, (PAYLOAD_BYTE, batch_size))).await;
            }
            await_delivery(&counter, target).await;
        });
    });

    rt.block_on(async {
        sender.node().shut_down().await;
        receiver.node().shut_down().await;
    });
}

/// Brings up a counting receiver and returns it alongside its shared counter and
/// listening address. Kept off the timed path.
async fn spawn_receiver(name: &str) -> (Receiver, Arc<AtomicUsize>, SocketAddr) {
    let counter = Arc::new(AtomicUsize::new(0));
    let receiver = Receiver {
        node: named_node(name),
        counter: counter.clone(),
    };
    receiver.enable_reading().await;
    let addr = receiver.node().toggle_listener().await.unwrap().unwrap();
    (receiver, counter, addr)
}

/// Enables writing, connects, and lets the link settle before timing starts.
async fn connect<W: Writing>(sender: &W, addr: SocketAddr) {
    sender.enable_writing().await;
    sender.node().connect(addr).await.unwrap();
    sleep(Duration::from_millis(100)).await;
}

/// Retries a `unicast_fast` enqueue, yielding on backpressure (`QuotaExceeded`).
async fn send(mut enqueue: impl FnMut() -> io::Result<()>) {
    loop {
        match enqueue() {
            Ok(_) => break,
            Err(e) if e.kind() == io::ErrorKind::QuotaExceeded => tokio::task::yield_now().await,
            Err(e) => panic!("unicast failed: {e}"),
        }
    }
}

/// Spins until the receiver has counted `target` bytes. On loopback there's no
/// loss, so this always converges; the deadline just keeps a wiring bug from
/// hanging the whole run silently.
async fn await_delivery(counter: &Arc<AtomicUsize>, target: usize) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while counter.load(Ordering::Relaxed) < target {
        if Instant::now() > deadline {
            panic!(
                "timed out waiting for delivery: {} / {}",
                counter.load(Ordering::Relaxed),
                target
            );
        }
        tokio::task::yield_now().await;
    }
}

fn runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}
