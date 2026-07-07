use std::{
    net::SocketAddr,
    sync::{
        Arc, LazyLock,
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

/// Messages sent per sender, per sample. The wire volume per sample is
/// `sender_count * NUM_MESSAGES * MSG_SIZE`; keep that in mind before raising
/// either this or `sample_count`, since every sample moves the full amount.
const NUM_MESSAGES: usize = 25_000;
/// The size of a single message; `BytesCodec` adds no framing, so this is also
/// the exact number of bytes each message puts on the wire.
const MSG_SIZE: usize = 32 * 1024;
/// Sender counts to sweep over (1 receiver throughout).
const SENDER_COUNTS: &[usize] = &[1, 10, 20, 50, 100];
/// The sender's outbound queue depth; every Nth message is awaited to delivery
/// so the bounded `unicast`/`unicast_fast` channel can't overflow.
const QUEUE_DEPTH: usize = <FullNoopNode as Writing>::MESSAGE_QUEUE_DEPTH;

/// A fixed payload. The content is irrelevant to throughput (nothing on the
/// wire is compressed and the receiver discards the bytes), so zeros are fine.
static PAYLOAD: LazyLock<Bytes> = LazyLock::new(|| Bytes::from(vec![0u8; MSG_SIZE]));

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

/// Aggregate inbound throughput at a single receiver as the number of
/// concurrent senders grows.
///
/// Nodes and connections are established once (untimed) and reused across
/// samples; each sample re-arms the receiver's counter and re-runs the blast,
/// so only the actual send/receive cycle is timed. The `BytesCount` counter
/// turns divan's per-sample time into a throughput figure.
#[divan::bench(args = SENDER_COUNTS, sample_count = 3, sample_size = 1)]
fn spam_to_one(bencher: Bencher, sender_count: usize) {
    let rt = runtime();
    let expected = sender_count * NUM_MESSAGES * MSG_SIZE;

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

            // every sender blasts NUM_MESSAGES concurrently
            let mut handles = Vec::with_capacity(sender_count);
            for sender in &senders {
                let sender = sender.clone();
                handles.push(tokio::spawn(async move {
                    for i in 0..NUM_MESSAGES {
                        if (i + 1) % QUEUE_DEPTH == 0 {
                            // sync point: wait until this message (and all
                            // prior ones) have been flushed to the socket
                            sender
                                .send_dm(receiver_addr, PAYLOAD.clone())
                                .await
                                .unwrap();
                        } else {
                            // fast path: queue without awaiting delivery
                            sender.unicast_fast(receiver_addr, PAYLOAD.clone()).unwrap();
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
