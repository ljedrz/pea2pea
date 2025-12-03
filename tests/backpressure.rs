use std::{
    io,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};

use bytes::{Bytes, BytesMut};
use deadline::deadline;
use tokio::time::sleep;

mod common;

use pea2pea::{
    Config, ConnectionSide, Node, Pea2Pea,
    protocols::{Reading, Writing},
};

#[derive(Clone)]
struct RealtimeNode {
    node: Node,
    // node: this is not the same as msgs_received in the Stats,
    // which is incremented even if the message is dropped due to
    // inbound queue saturation
    num_processed_messages: Arc<AtomicU8>,
}

impl Pea2Pea for RealtimeNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Reading for RealtimeNode {
    const MESSAGE_QUEUE_DEPTH: usize = 1;
    const BACKPRESSURE: bool = false;

    type Message = BytesMut;
    type Codec = common::TestCodec<BytesMut>;

    async fn process_message(&self, _: SocketAddr, _: Self::Message) {
        self.num_processed_messages.fetch_add(1, Ordering::Relaxed);
    }

    fn codec(&self, _: SocketAddr, _: ConnectionSide) -> <Self as Reading>::Codec {
        Default::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn drops_messages_wo_backpressure() {
    const MSGS_TO_SEND: u8 = 25;

    let rt_node = RealtimeNode {
        node: Node::new(Config {
            name: Some("realtime".into()),
            ..Default::default()
        }),
        num_processed_messages: Default::default(),
    };
    let fast_node = crate::test_node!("fastboi");

    rt_node.enable_reading().await;
    fast_node.enable_writing().await;

    let rt_node_addr = rt_node.node().toggle_listener().await.unwrap().unwrap();
    fast_node.node().connect(rt_node_addr).await.unwrap();

    for _ in 0..MSGS_TO_SEND {
        let _ = fast_node.unicast_fast(rt_node_addr, (&b"gotta go fast!"[..]).into());
    }

    // ensure that the realtime node has time to attempt to process everything
    let rt_node_clone = rt_node.clone();
    deadline!(Duration::from_secs(5), move || rt_node_clone
        .node()
        .stats()
        .received()
        .0
        == MSGS_TO_SEND as u64);

    // the number of processed messages should be lower than that of sent messages
    let processed = rt_node.num_processed_messages.load(Ordering::Relaxed);
    assert!(processed < MSGS_TO_SEND);
}

#[derive(Clone)]
struct SaturatedNode(Node);

impl Pea2Pea for SaturatedNode {
    fn node(&self) -> &Node {
        &self.0
    }
}

impl Writing for SaturatedNode {
    const MESSAGE_QUEUE_DEPTH: usize = 1;

    type Message = Bytes;
    type Codec = common::TestCodec<Bytes>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

#[tokio::test]
async fn outbound_queue_saturation() {
    let sender = SaturatedNode(Node::new(Config {
        name: Some("sender".into()),
        ..Default::default()
    }));
    let receiver = crate::test_node!("lazy_receiver");

    sender.enable_writing().await;
    // note: receiver does not enable reading, forcing TCP backpressure

    let receiver_addr = receiver.node().toggle_listener().await.unwrap().unwrap();
    sender.node().connect(receiver_addr).await.unwrap();

    // give the connection a moment to finalize
    sleep(Duration::from_millis(50)).await;

    let msg = Bytes::from(&b"spam"[..]);
    let mut saturated = false;

    // spam messages until the queue fills up
    for _ in 0..10 {
        match sender.unicast_fast(receiver_addr, msg.clone()) {
            Ok(_) => {
                // message queued successfully, keep spamming
            }
            Err(e) => {
                // validate that the error is indeed due to queue saturation
                if e.kind() == io::ErrorKind::QuotaExceeded {
                    saturated = true;
                    break;
                } else {
                    panic!("unexpected error type during saturation test: {e}");
                }
            }
        }
    }

    assert!(saturated);
}
