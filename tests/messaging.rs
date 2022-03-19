use bytes::{Buf, Bytes, BytesMut};
use parking_lot::Mutex;
use tokio::time::sleep;
use tokio_util::codec::Decoder;
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

impl Decoder for common::TestCodec<TestMessage> {
    type Item = TestMessage;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        Ok(self.0.decode(src)?.map(|mut bytes| bytes.get_u8().into()))
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
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr) -> Self::Codec {
        Default::default()
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
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr) -> Self::Codec {
        Default::default()
    }
}

#[tokio::test]
async fn messaging_example() {
    let shouter = crate::test_node!("shout");
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
    let reader = crate::test_node!("reader");
    let reader_addr = reader.node().listening_addr().unwrap();
    reader.enable_reading().await;

    let writer = crate::test_node!("writer");
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
async fn drop_connection_on_zero_read() {
    let reader = crate::test_node!("reader");
    reader.enable_reading().await;
    let peer = crate::test_node!("peer");

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
    let reader = crate::test_node!("defunct reader");
    let reader_addr = reader.node().listening_addr().unwrap();

    let writer = crate::test_node!("writer");
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
    let reader = crate::test_node!("reader");
    let reader_addr = reader.node().listening_addr().unwrap();
    reader.enable_reading().await;

    let writer = crate::test_node!("defunct writer");

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
