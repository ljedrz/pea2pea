use std::{
    io,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};

use bytes::BytesMut;
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

    async fn process_message(&self, _: SocketAddr, _: Self::Message) -> io::Result<()> {
        self.num_processed_messages.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn codec(&self, _: SocketAddr, _: ConnectionSide) -> <Self as Reading>::Codec {
        Default::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn drops_messages_wo_backpressure() {
    const MSGS_TO_SEND: u8 = 10;

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

    rt_node.node().toggle_listener().await.unwrap();
    let rt_node_addr = rt_node.node().listening_addr().await.unwrap();
    fast_node.node().connect(rt_node_addr).await.unwrap();

    for _ in 0..MSGS_TO_SEND {
        fast_node
            .unicast(rt_node_addr, (&b"gotta go fast!"[..]).into())
            .unwrap();
    }

    // ensure that the realtime node has time to attempt to process everything
    sleep(Duration::from_millis(100)).await;

    // the number of processed messages should be lower than that of sent messages
    let processed = rt_node.num_processed_messages.load(Ordering::Relaxed);
    assert!(processed < MSGS_TO_SEND);
}
