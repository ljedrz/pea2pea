//! Aggregate inbound throughput at a single receiver.
//!
//! Each timed sample blasts a fixed wire volume from `N` concurrent senders
//! into one reading node; divan's `BytesCount` turns per-sample time into a
//! bytes/s figure. Message sizes are swept against `Writing::INITIAL_BUFFER_SIZE`,
//! i.e. the backpressure boundary at which batched writes get flushed
//! mid-batch, so the sweep spans three regimes: well below it (a single write
//! many messages), around it, and above it (effectively one write per message).
//! Per-sender message counts scale inversely with size, keeping the wire
//! volume identical across cases.

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use bytes::{Bytes, BytesMut};
use divan::{Bencher, counter::BytesCount};
use pea2pea::{
    ConnectionSide, Node, Pea2Pea,
    protocols::{Reading, Writing},
};
use test_utils::{FullNoopNode, WritingExt, wait_for_connections};
use tokio::{runtime::Runtime, sync::Notify};
use tokio_util::codec::BytesCodec;

fn main() {
    divan::main();
}

/// The writer's backpressure boundary: outbound batches are flushed to the
/// socket whenever they reach this size, so it determines how deeply messages
/// coalesce into individual writes.
const BACKPRESSURE_BOUNDARY: usize = <FullNoopNode as Writing>::INITIAL_BUFFER_SIZE;
/// Wire volume moved by each sender in a single sample; per-case message counts
/// are derived from it, so every case shifts the same number of bytes. Keep
/// that in mind before raising either this or `sample_count`, since every
/// sample moves the full amount.
const SENDER_WIRE_BYTES: usize = 12_500 * 32 * 1024;
/// `(sender count, message size)` cases to sweep over (1 receiver throughout);
/// message sizes sit below (`/16`), at half of, and above (`*2`) the
/// backpressure boundary.
const CASES: &[(usize, usize)] = &[
    (1, BACKPRESSURE_BOUNDARY / 16),
    (1, BACKPRESSURE_BOUNDARY / 2),
    (1, BACKPRESSURE_BOUNDARY * 2),
    (10, BACKPRESSURE_BOUNDARY / 16),
    (10, BACKPRESSURE_BOUNDARY / 2),
    (10, BACKPRESSURE_BOUNDARY * 2),
    (20, BACKPRESSURE_BOUNDARY / 16),
    (20, BACKPRESSURE_BOUNDARY / 2),
    (20, BACKPRESSURE_BOUNDARY * 2),
    (50, BACKPRESSURE_BOUNDARY / 16),
    (50, BACKPRESSURE_BOUNDARY / 2),
    (50, BACKPRESSURE_BOUNDARY * 2),
    (100, BACKPRESSURE_BOUNDARY / 16),
    (100, BACKPRESSURE_BOUNDARY / 2),
    (100, BACKPRESSURE_BOUNDARY * 2),
];
/// The sender's outbound queue depth; every Nth message is awaited to delivery
/// so the bounded `unicast`/`unicast_fast` channel can't overflow. Delivery is
/// confirmed once the entire write batch containing the message has been
/// flushed, so the sync point also implies every prior message reached the
/// socket.
const QUEUE_DEPTH: usize = <FullNoopNode as Writing>::MESSAGE_QUEUE_DEPTH;

/// The receiver counts inbound bytes (`BytesCodec` chunks are arbitrary) and
/// fires `done` once `expected` of them have been processed.
#[derive(Clone)]
struct Receiver {
    node: Node,
    received: Arc<AtomicUsize>,
    expected: usize,
    done: Arc<Notify>,
}

impl Pea2Pea for Receiver {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Reading for Receiver {
    type Message = BytesMut;
    type Codec = BytesCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, _src: SocketAddr, msg: Self::Message) {
        if self.received.fetch_add(msg.len(), Ordering::Relaxed) + msg.len() == self.expected {
            self.done.notify_one();
        }
    }
}

/// Aggregate inbound throughput at a single receiver across `(senders, message
/// size)` cases; see the module docs for the batching rationale.
///
/// Nodes and connections are established once (untimed) and reused across
/// samples; each sample re-arms the receiver's counter and re-runs the blast,
/// so only the actual send/receive cycle is timed. The `BytesCount` counter
/// turns divan's per-sample time into a throughput figure.
#[divan::bench(args = CASES, sample_count = 3, sample_size = 1)]
fn spam_to_one(bencher: Bencher, case: (usize, usize)) {
    let (sender_count, msg_size) = case;
    let num_messages = SENDER_WIRE_BYTES / msg_size;
    let expected = sender_count * num_messages * msg_size;
    let rt = runtime();

    // A fixed payload; the content is irrelevant to throughput (nothing on the
    // wire is compressed and the receiver discards the bytes), so zeros are fine.
    let payload = Bytes::from(vec![0u8; msg_size]);

    // Untimed setup: a reading receiver and `sender_count` writing senders, all
    // connected and settled.
    let (receiver, senders, receiver_addr) = rt.block_on(async {
        let receiver = Receiver {
            node: Node::new(Default::default()),
            received: Arc::new(AtomicUsize::new(0)),
            expected,
            done: Arc::new(Notify::new()),
        };
        receiver.enable_reading().await;
        let receiver_addr = receiver.node().toggle_listener().await.unwrap().unwrap();

        let mut senders = Vec::with_capacity(sender_count);
        for _ in 0..sender_count {
            let sender = FullNoopNode::default();
            sender.enable_writing().await;
            sender.node().connect(receiver_addr).await.unwrap();
            senders.push(sender);
        }
        wait_for_connections(receiver.node(), sender_count).await;

        (receiver, senders, receiver_addr)
    });

    bencher.counter(BytesCount::new(expected)).bench_local(|| {
        rt.block_on(async {
            // re-arm for this sample
            receiver.received.store(0, Ordering::Relaxed);

            // every sender blasts its share of messages concurrently
            let mut handles = Vec::with_capacity(sender_count);
            for sender in &senders {
                let sender = sender.clone();
                let payload = payload.clone();
                handles.push(tokio::spawn(async move {
                    for i in 0..num_messages {
                        if (i + 1) % QUEUE_DEPTH == 0 {
                            // sync point: wait until this batch (and all prior
                            // ones) have been flushed to the socket
                            sender
                                .send_dm(receiver_addr, payload.clone())
                                .await
                                .unwrap();
                        } else {
                            // fast path: queue without awaiting delivery
                            sender.unicast_fast(receiver_addr, payload.clone()).unwrap();
                        }
                    }
                }));
            }

            // the receiver signals once every message has been processed
            receiver.done.notified().await;

            for handle in handles {
                handle.await.unwrap();
            }
        });
    });

    // Untimed teardown.
    rt.block_on(async {
        receiver.node().shut_down().await;
        for sender in &senders {
            sender.node().shut_down().await;
        }
    });
}

fn runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}
