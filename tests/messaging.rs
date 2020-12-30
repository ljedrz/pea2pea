use parking_lot::Mutex;
use tracing::*;

mod common;
use pea2pea::{Messaging, Node, NodeConfig, Pea2Pea};
use TestMessage::*;

use std::{collections::HashSet, io, net::SocketAddr, sync::Arc};

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
    node: Arc<Node>,
    echoed: Arc<Mutex<HashSet<TestMessage>>>,
}

impl Pea2Pea for EchoNode {
    fn node(&self) -> &Arc<Node> {
        &self.node
    }
}

#[async_trait::async_trait]
impl Messaging for EchoNode {
    type Message = TestMessage;

    fn read_message(buffer: &[u8]) -> io::Result<Option<(Self::Message, usize)>> {
        let bytes = common::read_len_prefixed_message(2, buffer)?;

        Ok(bytes.map(|bytes| (TestMessage::from(bytes[2]), bytes.len())))
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()> {
        info!(parent: self.node().span(), "got a {:?} from {}", message, source);

        if self.echoed.lock().insert(message) {
            info!(parent: self.node().span(), "it was new! echoing it");

            self.node()
                .send_direct_message(source, common::prefix_with_len(2, &[message as u8]))
                .await
                .unwrap();
        } else {
            debug!(parent: self.node().span(), "I've already heard {:?}! not echoing", message);
        }

        Ok(())
    }
}

#[tokio::test]
async fn messaging_example() {
    tracing_subscriber::fmt::init();

    let shouter = common::MessagingNode::new("shout").await;
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

    wait_until!(1, picky_echo.node().num_connected() == 1);

    shouter
        .node()
        .send_direct_message(picky_echo_addr, common::prefix_with_len(2, &[Herp as u8]))
        .await
        .unwrap();
    shouter
        .node()
        .send_direct_message(picky_echo_addr, common::prefix_with_len(2, &[Derp as u8]))
        .await
        .unwrap();
    shouter
        .node()
        .send_direct_message(picky_echo_addr, common::prefix_with_len(2, &[Herp as u8]))
        .await
        .unwrap();

    // let echo send one message on its own too, for good measure
    let shouter_addr = picky_echo.node().connected_addrs()[0];

    picky_echo
        .node()
        .send_direct_message(shouter_addr, common::prefix_with_len(2, &[Herp as u8]))
        .await
        .unwrap();

    // check if the shouter heard the (non-duplicate) echoes and the last, non-reply one
    wait_until!(1, shouter.node().stats.received().0 == 3);
}

#[tokio::test]
async fn drop_connection_on_invalid_message() {
    let writer = common::MessagingNode::new("writer").await;
    let reader = common::MessagingNode::new("reader").await;
    reader.enable_messaging();

    writer
        .node()
        .initiate_connection(reader.node().listening_addr)
        .await
        .unwrap();

    wait_until!(1, reader.node().num_connected() == 1);

    // an invalid message: a header indicating a zero-length payload
    let bad_message: &'static [u8] = &[0, 0];

    writer
        .node()
        .send_direct_message(reader.node().listening_addr, bad_message.into())
        .await
        .unwrap();

    wait_until!(1, reader.node().num_connected() == 0);
}

#[tokio::test]
async fn drop_connection_on_oversized_message() {
    const MSG_SIZE_LIMIT: usize = 10;

    let writer = common::MessagingNode::new("writer").await;

    let mut config = NodeConfig::default();
    config.name = Some("reader".into());
    config.conn_read_buffer_size = MSG_SIZE_LIMIT;
    let reader = common::MessagingNode(Node::new(Some(config)).await.unwrap());
    reader.enable_messaging();

    writer
        .node()
        .initiate_connection(reader.node().listening_addr)
        .await
        .unwrap();

    wait_until!(1, reader.node().num_connected() == 1);

    // when prefixed with length, it'll exceed MSG_SIZE_LIMIT, i.e. the read buffer size of the reader
    let oversized_payload = vec![0u8; MSG_SIZE_LIMIT];

    writer
        .node()
        .send_direct_message(
            reader.node().listening_addr,
            common::prefix_with_len(2, &oversized_payload),
        )
        .await
        .unwrap();

    wait_until!(1, reader.node().num_connected() == 0);
}
