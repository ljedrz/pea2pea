use bytes::Bytes;
use parking_lot::Mutex;
use tokio::time::sleep;
use tracing::*;

mod common;
use pea2pea::{ContainsNode, Messaging, Node, NodeConfig};

use std::{collections::HashSet, convert::TryInto, io, net::SocketAddr, sync::Arc, time::Duration};

#[derive(PartialEq, Eq, Hash, Debug, Clone, Copy)]
enum TestMessage {
    Herp,
    Derp,
}

#[derive(Clone)]
struct EchoNode {
    node: Arc<Node>,
    echoed: Arc<Mutex<HashSet<TestMessage>>>,
}

impl ContainsNode for EchoNode {
    fn node(&self) -> &Arc<Node> {
        &self.node
    }
}

fn packet_message(message: TestMessage) -> Bytes {
    let mut bytes = Vec::with_capacity(3);
    let u16_len = 1u16.to_le_bytes();
    bytes.extend_from_slice(&u16_len);
    bytes.push(message as u8);

    bytes.into()
}

#[async_trait::async_trait]
impl Messaging for EchoNode {
    type Message = TestMessage;

    fn read_message(buffer: &[u8]) -> io::Result<Option<&[u8]>> {
        // expecting the test messages to be prefixed with their length encoded as a LE u16
        if buffer.len() >= 2 {
            let payload_len = u16::from_le_bytes(buffer[..2].try_into().unwrap()) as usize;

            if payload_len == 0 {
                return Err(io::ErrorKind::InvalidData.into());
            }

            if buffer[2..].len() >= payload_len {
                Ok(Some(&buffer[..2 + payload_len]))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    fn parse_message(&self, _source: SocketAddr, message: Vec<u8>) -> Option<Self::Message> {
        // the first 2B are the u16 length, last one is the payload
        if message.len() == 3 {
            match message[2] {
                0 => Some(TestMessage::Herp),
                1 => Some(TestMessage::Derp),
                _ => None,
            }
        } else {
            None
        }
    }

    async fn respond_to_message(
        &self,
        source: SocketAddr,
        message: Self::Message,
    ) -> io::Result<()> {
        info!(parent: self.node().span(), "got a {:?} from {}", message, source);
        if self.echoed.lock().insert(message) {
            info!(parent: self.node().span(), "it was new! echoing it");

            self.node()
                .send_direct_message(source, packet_message(message))
                .await
                .unwrap();
        } else {
            debug!(parent: self.node().span(), "I've already heard {:?}! not echoing", message);
        }

        Ok(())
    }
}

#[tokio::test]
async fn messaging_protocol() {
    tracing_subscriber::fmt::init();

    let shouter = common::RandomNode::new("shout").await;
    shouter.enable_messaging();

    let mut picky_echo_config = NodeConfig::default();
    picky_echo_config.name = Some("picky_echo".into());
    let picky_echo = EchoNode {
        node: Node::new(Some(picky_echo_config)).await.unwrap(),
        echoed: Default::default(),
    };

    picky_echo.enable_messaging();

    let picky_echo_addr = picky_echo.node().listening_addr;

    shouter
        .node()
        .initiate_connection(picky_echo_addr)
        .await
        .unwrap();
    sleep(Duration::from_millis(100)).await;

    shouter
        .send_direct_message_with_len(picky_echo_addr, &[TestMessage::Herp as u8])
        .await;
    shouter
        .send_direct_message_with_len(picky_echo_addr, &[TestMessage::Derp as u8])
        .await;
    shouter
        .send_direct_message_with_len(picky_echo_addr, &[TestMessage::Herp as u8])
        .await;

    // let echo send one message on its own too, for good measure
    let shouter_addr = picky_echo.node().handshaken_addrs()[0];

    picky_echo
        .node()
        .send_direct_message(shouter_addr, packet_message(TestMessage::Herp))
        .await
        .unwrap();

    sleep(Duration::from_millis(100)).await;
    // check if the shouter heard the (non-duplicate) echoes and the last, non-reply one
    assert_eq!(shouter.node().num_messages_received(), 3);
}

#[tokio::test]
async fn drop_connection_on_invalid_message() {
    let writer = common::RandomNode::new("writer").await;
    let reader = common::RandomNode::new("reader").await;
    reader.enable_messaging();

    writer
        .node()
        .initiate_connection(reader.node().listening_addr)
        .await
        .unwrap();

    sleep(Duration::from_millis(10)).await;
    assert!(reader.node().num_connected() == 1);

    // an invalid message: a header indicating a zero-length payload
    let bad_message: &'static [u8] = &[0, 0];

    writer
        .node()
        .send_direct_message(reader.node().listening_addr, bad_message.into())
        .await
        .unwrap();
    sleep(Duration::from_millis(10)).await;

    assert!(reader.node().num_connected() == 0);
}

#[tokio::test]
async fn drop_connection_on_oversized_message() {
    const MSG_SIZE_LIMIT: usize = 10;

    let writer = common::RandomNode::new("writer").await;

    let mut config = NodeConfig::default();
    config.name = Some("reader".into());
    config.conn_read_buffer_size = MSG_SIZE_LIMIT;
    let reader = common::RandomNode(Node::new(Some(config)).await.unwrap());
    reader.enable_messaging();

    writer
        .node()
        .initiate_connection(reader.node().listening_addr)
        .await
        .unwrap();

    sleep(Duration::from_millis(10)).await;
    assert!(reader.node().num_connected() == 1);

    // when prefixed with length, it'll exceed MSG_SIZE_LIMIT, i.e. the read buffer size of the reader
    let oversized_payload = vec![0u8; MSG_SIZE_LIMIT];

    writer
        .send_direct_message_with_len(reader.node().listening_addr, &oversized_payload)
        .await;
    sleep(Duration::from_millis(10)).await;

    assert!(reader.node().num_connected() == 0);
}
