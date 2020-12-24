use parking_lot::Mutex;
use tokio::{io::AsyncReadExt, time::sleep};
use tracing::*;

mod common;
use pea2pea::{
    ConnectionReader, ContainsNode, MessagingProtocol, Node, NodeConfig, PacketingProtocol,
};

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

#[async_trait::async_trait]
impl MessagingProtocol for EchoNode {
    type Message = TestMessage;

    async fn read_message(connection_reader: &mut ConnectionReader) -> std::io::Result<Vec<u8>> {
        // expecting the test messages to be prefixed with their length encoded as a LE u16
        let msg_len_size: usize = 2;

        let buffer = &mut connection_reader.buffer;
        connection_reader
            .reader
            .read_exact(&mut buffer[..msg_len_size])
            .await?;
        let msg_len = u16::from_le_bytes(buffer[..msg_len_size].try_into().unwrap()) as usize;
        connection_reader
            .reader
            .read_exact(&mut buffer[..msg_len])
            .await?;

        Ok(buffer[..msg_len].to_vec())
    }

    fn parse_message(_source: SocketAddr, buffer: &[u8]) -> Option<Self::Message> {
        if buffer.len() == 1 {
            match buffer[0] {
                0 => Some(TestMessage::Herp),
                1 => Some(TestMessage::Derp),
                _ => None,
            }
        } else {
            None
        }
    }

    fn respond_to_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()> {
        info!(parent: self.node().span(), "got a {:?} from {}", message, source);
        if self.echoed.lock().insert(message) {
            info!(parent: self.node().span(), "it was new! echoing it");

            let self_clone = self.clone();
            tokio::spawn(async move {
                self_clone
                    .node()
                    .send_direct_message(source, vec![message as u8])
                    .await
                    .unwrap();
            });
        } else {
            debug!(parent: self.node().span(), "I've already heard {:?}! not echoing", message);
        }

        Ok(())
    }
}

impl PacketingProtocol for EchoNode {
    fn enable_packeting_protocol(&self) {
        self.node()
            .set_packeting_closure(Box::new(common::packeting_closure));
    }
}

#[tokio::test]
async fn messaging_protocol() {
    tracing_subscriber::fmt::init();

    let shouter = common::RandomNode::new("shout").await;
    shouter.enable_messaging_protocol();
    shouter.enable_packeting_protocol();

    let mut picky_echo_config = NodeConfig::default();
    picky_echo_config.name = Some("picky_echo".into());
    let picky_echo = Arc::new(EchoNode {
        node: Node::new(Some(picky_echo_config)).await.unwrap(),
        echoed: Default::default(),
    });

    picky_echo.enable_messaging_protocol();
    picky_echo.enable_packeting_protocol();

    let picky_echo_addr = picky_echo.node().listening_addr;

    shouter
        .node()
        .initiate_connection(picky_echo_addr)
        .await
        .unwrap();

    shouter
        .node()
        .send_direct_message(picky_echo_addr, vec![TestMessage::Herp as u8])
        .await
        .unwrap();
    shouter
        .node()
        .send_direct_message(picky_echo_addr, vec![TestMessage::Derp as u8])
        .await
        .unwrap();
    shouter
        .node()
        .send_direct_message(picky_echo_addr, vec![TestMessage::Herp as u8])
        .await
        .unwrap();

    sleep(Duration::from_millis(200)).await;
    // check if the shouter heard the (non-duplicate) echoes
    assert_eq!(shouter.node().num_messages_received(), 2);
}
