//! Measures Packets Per Second (PPS).
//!
//! Modes:
//! 1. **Naive**: Sends `u8` one by one. Measures sender's internal architecture.
//! 2. **Batch**: Blasts `N` copies of `u8` into the buffer. Measures receiver's ingress.

mod common;

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
use pea2pea::{
    Config, ConnectionSide, Node, Pea2Pea,
    protocols::{Reading, Writing},
};
use tokio::time::sleep;
use tokio_util::codec::{Decoder, Encoder};

const BENCH_DURATION: Duration = Duration::from_secs(5);
const PAYLOAD_BYTE: u8 = b'x';
const BATCH_SIZE: usize = 100;

// --- codecs ---

/// a raw codec that treats every single byte as a message
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

/// a codec that expands a single (byte, count) tuple into N raw bytes
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

// --- receiver ---

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

    fn process_message(
        &self,
        _source: SocketAddr,
        _message: Self::Message,
    ) -> impl std::future::Future<Output = ()> + Send {
        self.counter.fetch_add(1, Ordering::Relaxed);
        async {}
    }
}

impl Writing for Receiver {
    type Message = u8;
    type Codec = RawByteCodec;
    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

// --- naive sender ---

#[derive(Clone)]
struct NaiveSender {
    node: Node,
}

impl Pea2Pea for NaiveSender {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Reading for NaiveSender {
    type Message = u8;
    type Codec = RawByteCodec;
    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
    async fn process_message(&self, _source: SocketAddr, _message: Self::Message) {}
}

impl Writing for NaiveSender {
    type Message = u8;
    type Codec = RawByteCodec;
    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

// --- batch sender ---

#[derive(Clone)]
struct BatchSender {
    node: Node,
}

impl Pea2Pea for BatchSender {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Reading for BatchSender {
    type Message = u8;
    type Codec = RawByteCodec;
    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
    async fn process_message(&self, _source: SocketAddr, _message: Self::Message) {}
}

impl Writing for BatchSender {
    type Message = (u8, usize);
    type Codec = BatchRawCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        BatchRawCodec
    }
}

// --- scenarios ---

async fn run_naive_test() {
    println!("\n--- MODE 1: Naive (1 Unicast = 1 Byte) ---");
    let counter = Arc::new(AtomicUsize::new(0));
    let receiver = Receiver {
        node: Node::new(Config {
            name: Some("recv_naive".into()),
            ..Default::default()
        }),
        counter: counter.clone(),
    };
    receiver.enable_reading().await;
    receiver.enable_writing().await;
    let addr = receiver.node().toggle_listener().await.unwrap().unwrap();

    let sender = NaiveSender {
        node: Node::new(Config {
            name: Some("send_naive".into()),
            ..Default::default()
        }),
    };
    sender.enable_writing().await;
    sender.node().connect(addr).await.unwrap();
    sleep(Duration::from_millis(100)).await;

    let start = Instant::now();
    let sender_clone = sender.clone();

    let handle = tokio::spawn(async move {
        while start.elapsed() < BENCH_DURATION {
            match sender_clone.unicast(addr, PAYLOAD_BYTE) {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::Other => tokio::task::yield_now().await,
                Err(_) => break,
            }
        }
    });

    monitor(counter.clone(), start).await;
    let _ = handle.await;

    report("Naive", counter.load(Ordering::Relaxed));
    sender.node().shut_down().await;
    receiver.node().shut_down().await;
}

async fn run_batch_test() {
    println!("\n--- MODE 2: Batch (1 Unicast = {} Bytes) ---", BATCH_SIZE);
    let counter = Arc::new(AtomicUsize::new(0));
    let receiver = Receiver {
        node: Node::new(Config {
            name: Some("recv_batch".into()),
            ..Default::default()
        }),
        counter: counter.clone(),
    };
    receiver.enable_reading().await;
    receiver.enable_writing().await;
    let addr = receiver.node().toggle_listener().await.unwrap().unwrap();

    let sender = BatchSender {
        node: Node::new(Config {
            name: Some("send_batch".into()),
            ..Default::default()
        }),
    };
    sender.enable_writing().await;
    sender.node().connect(addr).await.unwrap();
    sleep(Duration::from_millis(100)).await;

    let start = Instant::now();
    let sender_clone = sender.clone();

    let handle = tokio::spawn(async move {
        while start.elapsed() < BENCH_DURATION {
            match sender_clone.unicast(addr, (PAYLOAD_BYTE, BATCH_SIZE)) {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::Other => tokio::task::yield_now().await,
                Err(_) => break,
            }
        }
    });

    monitor(counter.clone(), start).await;
    let _ = handle.await;

    report("Batch", counter.load(Ordering::Relaxed));
    sender.node().shut_down().await;
    receiver.node().shut_down().await;
}

async fn monitor(counter: Arc<AtomicUsize>, start: Instant) {
    let mut last_count = 0;
    let mut last_tick = Instant::now();

    while start.elapsed() < BENCH_DURATION {
        sleep(Duration::from_millis(1000)).await;
        let now = Instant::now();
        let total = counter.load(Ordering::Relaxed);
        let delta = total - last_count;
        let elapsed = now.duration_since(last_tick).as_secs_f64();
        println!(
            "[{:.0}s] Rate: {:>8.0} PPS",
            start.elapsed().as_secs_f64(),
            delta as f64 / elapsed
        );
        last_count = total;
        last_tick = now;
    }
}

fn report(mode: &str, total: usize) {
    let pps = total as f64 / BENCH_DURATION.as_secs_f64();
    let mbs = pps / 1_000_000.0;

    println!("--- {} Results ---", mode);
    println!("Total Packets: {}", total);
    println!("Average PPS:   {:.0}", pps);
    println!("Throughput:    {:.2} MB/s (wire)", mbs);
}

#[tokio::main]
async fn main() {
    run_naive_test().await;
    sleep(Duration::from_secs(1)).await;
    run_batch_test().await;
}
