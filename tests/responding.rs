use parking_lot::Mutex;
use tokio::{io::AsyncReadExt, sync::mpsc::channel, time::sleep};
use tracing::*;

mod common;
use pea2pea::{
    ConnectionReader, ContainsNode, Node, NodeConfig, ReadProtocol, ResponseProtocol, WriteProtocol,
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
    fn node(&self) -> &Node {
        &self.node
    }
}

impl_read_protocol!(EchoNode);
impl WriteProtocol for EchoNode {}

impl ResponseProtocol for EchoNode {
    type Message = TestMessage;

    fn enable_response_protocol(self: &Arc<Self>) {
        let (sender, mut receiver) = channel(4);
        self.node().set_incoming_requests(sender);

        let node = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                if let Some((request, source)) = receiver.recv().await {
                    if let Some(msg) = node.parse_message(&request) {
                        node.process_message(&msg);

                        if let Err(e) = node.respond_to_message(msg, source) {
                            error!(parent: node.span(), "failed to handle an incoming message: {}", e);
                        }
                    } else {
                        error!(parent: node.span(), "can't parse an incoming message");
                    }
                }
            }
        });
    }

    fn parse_message(&self, buffer: &[u8]) -> Option<Self::Message> {
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

    fn respond_to_message(
        self: &Arc<Self>,
        message: TestMessage,
        source_addr: SocketAddr,
    ) -> io::Result<()> {
        info!(parent: self.span(), "got a {:?} from {}", message, source_addr);
        if self.echoed.lock().insert(message) {
            info!(parent: self.span(), "it was new! echoing it");

            let node = Arc::clone(self);
            tokio::spawn(async move {
                node.send_direct_message(source_addr, vec![message as u8])
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
async fn request_handling_echo() {
    tracing_subscriber::fmt::init();

    let shout_node = common::RwNode::new().await;

    let writing_closure = Box::new(|message: &[u8]| -> Vec<u8> {
        let mut message_with_u16_len = Vec::with_capacity(message.len() + 2);
        message_with_u16_len.extend_from_slice(&(message.len() as u16).to_le_bytes());
        message_with_u16_len.extend_from_slice(message);
        message_with_u16_len
    });

    shout_node.enable_writing_protocol(writing_closure.clone());

    let mut echo_node_config = NodeConfig::default();
    echo_node_config.name = Some("echo".into());
    let echo_node = Arc::new(EchoNode {
        node: Node::new(Some(echo_node_config)).await.unwrap(),
        echoed: Default::default(),
    });

    echo_node.enable_reading_protocol();
    echo_node.enable_writing_protocol(writing_closure);
    echo_node.enable_response_protocol();

    shout_node
        .initiate_connection(echo_node.local_addr)
        .await
        .unwrap();

    shout_node
        .send_direct_message(echo_node.local_addr, vec![TestMessage::Herp as u8])
        .await
        .unwrap();
    shout_node
        .send_direct_message(echo_node.local_addr, vec![TestMessage::Derp as u8])
        .await
        .unwrap();
    shout_node
        .send_direct_message(echo_node.local_addr, vec![TestMessage::Herp as u8])
        .await
        .unwrap();

    sleep(Duration::from_millis(200)).await;
}
