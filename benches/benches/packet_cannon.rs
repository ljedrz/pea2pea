//! Packets-per-second (PPS) throughput.
//!
//! Each timed sample sends a fixed number of packets end-to-end and waits for
//! the receiver to count every one; an `ItemsCount` counter turns the per-sample
//! time back into PPS, so divan reports throughput directly. Two send paths:
//!
//! * **naive** - one `unicast_fast` call per byte.
//! * **batch** - one `unicast_fast` call expands to `N` bytes via the codec, so
//!   the same payload goes out in `1/N` as many calls. The arg sweeps `N`.
//!
//! The receiver decodes byte-by-byte in both cases, so the receive-side work is
//! identical; the only difference is how many send calls produce it.
//!
//! Read the median rather than the mean; under load the batch rows can grow a
//! right tail from scheduler jitter while the medians stay put.

use std::{
    io,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use bytes::{Buf, BufMut, BytesMut};
use divan::{Bencher, counter::ItemsCount};
use pea2pea::{
    Config, ConnectionSide, Node, Pea2Pea,
    protocols::{Reading, Writing},
};
use test_utils::wait_for_connections;
use tokio::{runtime::Runtime, sync::Notify, time::timeout};
use tokio_util::codec::{Decoder, Encoder};

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

/// Reading-only node that counts every byte it decodes and fires `done` once a
/// full sample's worth (`PACKETS`) of them has been processed.
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
    type Message = u8;
    type Codec = RawByteCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, _source: SocketAddr, _message: Self::Message) {
        // fetch_add is the single crossing point per boundary, so exactly one
        // message per sample trips the signal even under concurrent dispatch
        if (self.counter.fetch_add(1, Ordering::Relaxed) + 1).is_multiple_of(PACKETS) {
            self.done.notify_one();
        }
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
    let (receiver, addr) = rt.block_on(spawn_receiver());

    let sender = NaiveSender {
        node: Node::new(Config::default()),
    };
    rt.block_on(connect(&sender, receiver.node(), addr));

    bencher.counter(ItemsCount::new(PACKETS)).bench_local(|| {
        rt.block_on(async {
            for _ in 0..PACKETS {
                send(|| sender.unicast_fast(addr, PAYLOAD_BYTE)).await;
            }
            await_batch(&receiver.done).await;
        });
    });

    rt.block_on(async {
        sender.node().shut_down().await;
        receiver.node().shut_down().await;
    });
}

/// Same delivered payload, but coalesced into `PACKETS / batch_size` writes.
#[divan::bench(sample_count = 100, sample_size = 1, args = [10, 100, 1000])]
fn batch(bencher: Bencher, batch_size: usize) {
    assert_eq!(
        PACKETS % batch_size,
        0,
        "PACKETS must divide evenly by batch_size"
    );

    let rt = runtime();
    let (receiver, addr) = rt.block_on(spawn_receiver());

    let sender = BatchSender {
        node: Node::new(Config::default()),
    };
    rt.block_on(connect(&sender, receiver.node(), addr));

    let frames = PACKETS / batch_size;

    bencher.counter(ItemsCount::new(PACKETS)).bench_local(|| {
        rt.block_on(async {
            for _ in 0..frames {
                send(|| sender.unicast_fast(addr, (PAYLOAD_BYTE, batch_size))).await;
            }
            await_batch(&receiver.done).await;
        });
    });

    rt.block_on(async {
        sender.node().shut_down().await;
        receiver.node().shut_down().await;
    });
}

/// Brings up a counting receiver and returns it alongside its listening address.
/// Kept off the timed path.
async fn spawn_receiver() -> (Receiver, SocketAddr) {
    let receiver = Receiver {
        node: Node::new(Config::default()),
        counter: Default::default(),
        done: Arc::new(Notify::new()),
    };
    receiver.enable_reading().await;
    let addr = receiver.node().toggle_listener().await.unwrap().unwrap();
    (receiver, addr)
}

/// Enables writing, connects, and waits until the receiver has registered the
/// link (and is thus reading from it) before timing starts.
async fn connect<W: Writing>(sender: &W, receiver: &Node, addr: SocketAddr) {
    sender.enable_writing().await;
    sender.node().connect(addr).await.unwrap();
    wait_for_connections(receiver, 1).await;
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
