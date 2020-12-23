use parking_lot::Mutex;
use tokio::{io::AsyncReadExt, time::sleep};
use tracing::*;

mod common;
use pea2pea::{
    ConnectionReader, ContainsNode, MessagingProtocol, Node, NodeConfig, PacketingProtocol,
};

use std::{
    collections::HashSet, convert::TryInto, io, net::SocketAddr, ops::Deref, sync::Arc,
    time::Duration,
};

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

impl Deref for EchoNode {
    type Target = Node;

    fn deref(&self) -> &Self::Target {
        &self.node
    }
}

impl ContainsNode for EchoNode {
    fn node(&self) -> &Arc<Node> {
        &self.node
    }
}

impl PacketingProtocol for EchoNode {}

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
        info!(parent: self.span(), "got a {:?} from {}", message, source);
        if self.echoed.lock().insert(message) {
            info!(parent: self.span(), "it was new! echoing it");

            let node = self.clone();
            tokio::spawn(async move {
                node.send_direct_message(source, vec![message as u8])
                    .await
                    .unwrap();
            });
        } else {
            debug!(parent: self.span(), "I've already seen {:?}! not echoing", message);
        }

        Ok(())
    }
}

#[tokio::test]
async fn echo_node_messaging() {
    tracing_subscriber::fmt::init();

    let shout_node = common::GenericNode::new().await;

    let packeting_closure = Box::new(|message: &[u8]| -> Vec<u8> {
        let mut message_with_u16_len = Vec::with_capacity(message.len() + 2);
        message_with_u16_len.extend_from_slice(&(message.len() as u16).to_le_bytes());
        message_with_u16_len.extend_from_slice(message);
        message_with_u16_len
    });

    shout_node.enable_packeting_protocol(packeting_closure.clone());

    let mut echo_node_config = NodeConfig::default();
    echo_node_config.name = Some("echo".into());
    let echo_node = Arc::new(EchoNode {
        node: Node::new(Some(echo_node_config)).await.unwrap(),
        echoed: Default::default(),
    });

    echo_node.enable_messaging_protocol();
    echo_node.enable_packeting_protocol(packeting_closure);

    shout_node
        .initiate_connection(echo_node.listening_addr)
        .await
        .unwrap();

    shout_node
        .send_direct_message(echo_node.listening_addr, vec![TestMessage::Herp as u8])
        .await
        .unwrap();
    shout_node
        .send_direct_message(echo_node.listening_addr, vec![TestMessage::Derp as u8])
        .await
        .unwrap();
    shout_node
        .send_direct_message(echo_node.listening_addr, vec![TestMessage::Herp as u8])
        .await
        .unwrap();

    sleep(Duration::from_millis(200)).await;
}
