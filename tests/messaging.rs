use bytes::Bytes;
use parking_lot::Mutex;
use tokio::time::sleep;
use tracing::*;

mod common;
use pea2pea::{
    protocols::{Reading, Writing},
    Node, NodeConfig, Pea2Pea,
};
use TestMessage::*;

use std::{collections::HashSet, io, net::SocketAddr, sync::Arc, time::Duration};

#[derive(PartialEq, Eq, Hash, Debug, Clone, Copy)]
enum TestMessage {
    Herp,
    Derp,
}

impl From<u8> for TestMessage {
    fn from(byte: u8) -> Self {
        match byte {
            0 => Self::Herp,
            1 => Self::Derp,
            _ => panic!("can't deserialize a TestMessage!"),
        }
    }
}

#[derive(Clone)]
struct EchoNode {
    node: Node,
    echoed: Arc<Mutex<HashSet<TestMessage>>>,
}

impl Pea2Pea for EchoNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

#[async_trait::async_trait]
impl Reading for EchoNode {
    type Message = TestMessage;

    fn read_message(
        &self,
        _source: SocketAddr,
        buffer: &[u8],
    ) -> io::Result<Option<(Self::Message, usize)>> {
        let bytes = common::read_len_prefixed_message(4, buffer)?;

        Ok(bytes.map(|bytes| (TestMessage::from(bytes[4]), bytes.len())))
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()> {
        info!(parent: self.node().span(), "got a {:?} from {}", message, source);

        if self.echoed.lock().insert(message) {
            info!(parent: self.node().span(), "it was new! echoing it");

            self.node()
                .send_direct_message(source, Bytes::copy_from_slice(&[message as u8]))
                .unwrap();
        } else {
            debug!(parent: self.node().span(), "I've already heard {:?}! not echoing", message);
        }

        Ok(())
    }
}

impl Writing for EchoNode {
    fn write_message(&self, _: SocketAddr, payload: &[u8], buffer: &mut [u8]) -> io::Result<usize> {
        buffer[..4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        buffer[4..][..payload.len()].copy_from_slice(payload);
        Ok(4 + payload.len())
    }
}

#[tokio::test]
async fn messaging_example() {
    // tracing_subscriber::fmt::init();

    let shouter = common::MessagingNode::new("shout").await;
    shouter.enable_reading();
    shouter.enable_writing();

    let picky_echo_config = NodeConfig {
        name: Some("picky_echo".into()),
        ..Default::default()
    };
    let picky_echo = EchoNode {
        node: Node::new(Some(picky_echo_config)).await.unwrap(),
        echoed: Default::default(),
    };
    picky_echo.enable_reading();
    picky_echo.enable_writing();

    let picky_echo_addr = picky_echo.node().listening_addr().unwrap();

    shouter.node().connect(picky_echo_addr).await.unwrap();

    wait_until!(1, picky_echo.node().num_connected() == 1);

    for message in &[Herp, Derp, Herp] {
        let msg = Bytes::copy_from_slice(&[*message as u8]);
        shouter
            .node()
            .send_direct_message(picky_echo_addr, msg)
            .unwrap();
    }

    // let echo send one message on its own too, for good measure
    let shouter_addr = picky_echo.node().connected_addrs()[0];

    picky_echo
        .node()
        .send_direct_message(shouter_addr, [Herp as u8][..].into())
        .unwrap();

    // check if the shouter heard the (non-duplicate) echoes and the last, non-reply one
    wait_until!(1, shouter.node().stats().received().0 == 3);
}

#[tokio::test]
async fn drop_connection_on_invalid_message() {
    let reader = common::MessagingNode::new("reader").await;
    let reader_addr = reader.node().listening_addr().unwrap();
    reader.enable_reading();

    let writer = common::MessagingNode::new("writer").await;
    writer.enable_writing();

    writer.node().connect(reader_addr).await.unwrap();

    wait_until!(1, reader.node().num_connected() == 1);

    // an invalid message: a zero-length payload
    let bad_message = Bytes::from(vec![]);

    writer
        .node()
        .send_direct_message(reader_addr, bad_message)
        .unwrap();

    wait_until!(1, reader.node().num_connected() == 0);
}

#[tokio::test]
async fn drop_connection_on_oversized_message() {
    const MSG_SIZE_LIMIT: usize = 10;

    let writer = common::MessagingNode::new("writer").await;
    writer.enable_writing();

    let config = NodeConfig {
        name: Some("reader".into()),
        conn_read_buffer_size: MSG_SIZE_LIMIT,
        ..Default::default()
    };
    let reader = common::MessagingNode(Node::new(Some(config)).await.unwrap());
    let reader_addr = reader.node().listening_addr().unwrap();
    reader.enable_reading();

    writer.node().connect(reader_addr).await.unwrap();

    wait_until!(1, reader.node().num_connected() == 1);

    // when prefixed with length, it'll exceed MSG_SIZE_LIMIT, i.e. the read buffer size of the reader
    let oversized_payload = vec![0u8; MSG_SIZE_LIMIT];

    writer
        .node()
        .send_direct_message(reader_addr, common::prefix_with_len(2, &oversized_payload))
        .unwrap();

    wait_until!(1, reader.node().num_connected() == 0);
}

#[tokio::test]
async fn drop_connection_on_zero_read() {
    let reader = common::MessagingNode::new("reader").await;
    reader.enable_reading();
    let peer = common::MessagingNode::new("peer").await;

    peer.node()
        .connect(reader.node().listening_addr().unwrap())
        .await
        .unwrap();

    wait_until!(1, reader.node().num_connected() == 1);

    // the peer shuts down, i.e. disconnects
    peer.node().shut_down().await;

    // the reader should drop its connection too now
    wait_until!(1, reader.node().num_connected() == 0);
}
