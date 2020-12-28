use parking_lot::Mutex;
use tracing::*;

mod common;
use pea2pea::{ContainsNode, Messaging, Node, NodeConfig};

use std::{collections::HashSet, io, net::SocketAddr, sync::Arc};

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

#[async_trait::async_trait]
impl Messaging for EchoNode {
    fn read_message(buffer: &[u8]) -> io::Result<Option<&[u8]>> {
        common::read_len_prefixed_message(2, buffer)
    }

    async fn process_message(&self, source: SocketAddr, message: Vec<u8>) -> io::Result<()> {
        // the first 2B are the u16 length, last one is the payload
        let deserialized_message = if message.len() == 3 {
            match message[2] {
                0 => TestMessage::Herp,
                1 => TestMessage::Derp,
                _ => return Err(io::ErrorKind::InvalidData.into()),
            }
        } else {
            return Err(io::ErrorKind::InvalidData.into());
        };

        info!(parent: self.node().span(), "got a {:?} from {}", deserialized_message, source);

        if self.echoed.lock().insert(deserialized_message) {
            info!(parent: self.node().span(), "it was new! echoing it");

            self.node()
                .send_direct_message(source, message.into())
                .await
                .unwrap();
        } else {
            debug!(parent: self.node().span(), "I've already heard {:?}! not echoing", deserialized_message);
        }

        Ok(())
    }
}

#[tokio::test]
async fn messaging_example() {
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

    wait_until!(1, picky_echo.node().num_connected() == 1);

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
    let shouter_addr = picky_echo.node().connected_addrs()[0];

    picky_echo
        .node()
        .send_direct_message(
            shouter_addr,
            common::prefix_message_with_len(2, &[TestMessage::Herp as u8]),
        )
        .await
        .unwrap();

    // check if the shouter heard the (non-duplicate) echoes and the last, non-reply one
    wait_until!(1, shouter.node().num_messages_received() == 3);
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

    wait_until!(1, reader.node().num_connected() == 1);

    // when prefixed with length, it'll exceed MSG_SIZE_LIMIT, i.e. the read buffer size of the reader
    let oversized_payload = vec![0u8; MSG_SIZE_LIMIT];

    writer
        .send_direct_message_with_len(reader.node().listening_addr, &oversized_payload)
        .await;

    wait_until!(1, reader.node().num_connected() == 0);
}
