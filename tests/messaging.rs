use bytes::{Buf, BufMut, Bytes};
use parking_lot::Mutex;
use tokio::time::sleep;
use tracing::*;

mod common;
use pea2pea::{
    protocols::{Reading, Writing},
    Config, Node, Pea2Pea,
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

    fn read_message<R: Buf>(
        &self,
        _source: SocketAddr,
        reader: &mut R,
    ) -> io::Result<Option<Self::Message>> {
        let byte = common::read_len_prefixed_message::<R, 2>(reader)?;
        Ok(byte.map(|byte| TestMessage::from(byte[0])))
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()> {
        info!(parent: self.node().span(), "got a {:?} from {}", message, source);

        if self.echoed.lock().insert(message) {
            info!(parent: self.node().span(), "it was new! echoing it");

            self.send_direct_message(source, Bytes::copy_from_slice(&[message as u8]))
                .unwrap()
                .await
                .unwrap();
        } else {
            debug!(parent: self.node().span(), "I've already heard {:?}! not echoing", message);
        }

        Ok(())
    }
}

impl Writing for EchoNode {
    type Message = Bytes;

    fn write_message<B: BufMut>(&self, _: SocketAddr, payload: &Self::Message, buffer: &mut B) {
        buffer.put_u16_le(payload.len() as u16);
        buffer.put_slice(payload);
    }
}

#[tokio::test]
async fn messaging_example() {
    // tracing_subscriber::fmt::init();

    let shouter = common::MessagingNode::new("shout").await;
    shouter.enable_reading().await;
    shouter.enable_writing().await;

    let picky_echo_config = Config {
        name: Some("picky_echo".into()),
        ..Default::default()
    };
    let picky_echo = EchoNode {
        node: Node::new(Some(picky_echo_config)).await.unwrap(),
        echoed: Default::default(),
    };
    picky_echo.enable_reading().await;
    picky_echo.enable_writing().await;

    let picky_echo_addr = picky_echo.node().listening_addr().unwrap();

    shouter.node().connect(picky_echo_addr).await.unwrap();

    wait_until!(1, picky_echo.node().num_connected() == 1);

    for message in &[Herp, Derp, Herp] {
        let msg = Bytes::copy_from_slice(&[*message as u8]);
        shouter
            .send_direct_message(picky_echo_addr, msg)
            .unwrap()
            .await
            .unwrap();
    }

    // let echo send one message on its own too, for good measure
    let shouter_addr = picky_echo.node().connected_addrs()[0];

    picky_echo
        .send_direct_message(shouter_addr, [Herp as u8][..].into())
        .unwrap()
        .await
        .unwrap();

    // check if the shouter heard the (non-duplicate) echoes and the last, non-reply one
    wait_until!(1, shouter.node().stats().received().0 == 3);
}

#[tokio::test]
async fn drop_connection_on_invalid_message() {
    let reader = common::MessagingNode::new("reader").await;
    let reader_addr = reader.node().listening_addr().unwrap();
    reader.enable_reading().await;

    let writer = common::MessagingNode::new("writer").await;
    writer.enable_writing().await;

    writer.node().connect(reader_addr).await.unwrap();

    wait_until!(1, reader.node().num_connected() == 1);

    // an invalid message: a zero-length payload
    let bad_message = Bytes::from(vec![]);

    writer
        .send_direct_message(reader_addr, bad_message)
        .unwrap()
        .await
        .unwrap();

    wait_until!(1, reader.node().num_connected() == 0);
}

#[tokio::test]
async fn drop_connection_on_oversized_message() {
    const MSG_SIZE_LIMIT: usize = 10;

    let writer = common::MessagingNode::new("writer").await;
    writer.enable_writing().await;

    let config = Config {
        name: Some("reader".into()),
        read_buffer_size: MSG_SIZE_LIMIT,
        ..Default::default()
    };
    let reader = common::MessagingNode(Node::new(Some(config)).await.unwrap());
    let reader_addr = reader.node().listening_addr().unwrap();
    reader.enable_reading().await;

    writer.node().connect(reader_addr).await.unwrap();

    wait_until!(1, reader.node().num_connected() == 1);

    // when prefixed with length, it'll be equal to MSG_SIZE_LIMIT
    let max_size_payload = vec![0u8; MSG_SIZE_LIMIT - 2];

    writer
        .send_direct_message(reader_addr, max_size_payload.into())
        .unwrap()
        .await
        .unwrap();

    wait_until!(1, reader.node().stats().received() == (1, 10));

    // this message exceeds MSG_SIZE_LIMIT, which is also the reader's read
    // buffer size, by a single byte
    let oversized_payload = vec![0u8; MSG_SIZE_LIMIT - 1];

    writer
        .send_direct_message(reader_addr, oversized_payload.into())
        .unwrap()
        .await
        .unwrap();

    wait_until!(1, reader.node().num_connected() == 0);
}

#[tokio::test]
async fn drop_connection_on_zero_read() {
    let reader = common::MessagingNode::new("reader").await;
    reader.enable_reading().await;
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

#[tokio::test]
async fn no_reading_no_delivery() {
    let reader = common::MessagingNode::new("defunct reader").await;
    let reader_addr = reader.node().listening_addr().unwrap();

    let writer = common::MessagingNode::new("writer").await;
    writer.enable_writing().await;

    writer.node().connect(reader_addr).await.unwrap();

    wait_until!(1, reader.node().num_connected() == 1);

    // writer sends a message
    writer
        .send_direct_message(reader_addr, vec![0; 16].into())
        .unwrap()
        .await
        .unwrap();

    sleep(Duration::from_millis(10)).await;

    // but the reader didn't enable reading, so it won't receive anything
    wait_until!(1, reader.node().stats().received() == (0, 0));
}

#[tokio::test]
async fn no_writing_no_delivery() {
    let reader = common::MessagingNode::new("reader").await;
    let reader_addr = reader.node().listening_addr().unwrap();
    reader.enable_reading().await;

    let writer = common::MessagingNode::new("defunct writer").await;

    writer.node().connect(reader_addr).await.unwrap();

    wait_until!(1, reader.node().num_connected() == 1);

    // writer tries to send a message
    assert!(writer
        .send_direct_message(reader_addr, vec![0; 16].into())
        .is_err());

    sleep(Duration::from_millis(10)).await;

    // the writer didn't enable writing, so the reader won't receive anything
    wait_until!(1, reader.node().stats().received() == (0, 0));
}
