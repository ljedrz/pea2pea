use parking_lot::Mutex;
use tokio::{io::AsyncReadExt, sync::mpsc::channel, time::sleep};
use tracing::*;

use pea2pea::{ConnectionReader, Node, NodeConfig, ReadProtocol, ResponseProtocol};

use std::{
    collections::HashSet, convert::TryInto, io, net::SocketAddr, ops::Deref, sync::Arc,
    time::Duration,
};

#[derive(PartialEq, Eq, Hash, Debug, Clone, Copy)]
enum TestMessage {
    Herp,
    Derp,
}

struct GenericNode(Arc<Node>);

impl Deref for GenericNode {
    type Target = Arc<Node>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
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

#[async_trait::async_trait]
impl ReadProtocol for GenericNode {
    fn node(&self) -> &Node {
        &self.0
    }

    async fn read_message(connection_reader: &mut ConnectionReader) -> io::Result<Vec<u8>> {
        let buffer = &mut connection_reader.buffer;
        connection_reader
            .reader
            .read_exact(&mut buffer[..2])
            .await?;
        let msg_len = u16::from_le_bytes(buffer[..2].try_into().unwrap()) as usize;
        connection_reader
            .reader
            .read_exact(&mut buffer[..msg_len])
            .await?;

        Ok(buffer[..msg_len].to_vec())
    }
}

#[async_trait::async_trait]
impl ReadProtocol for EchoNode {
    fn node(&self) -> &Node {
        &self.node
    }

    // FIXME: dedup impl
    async fn read_message(connection_reader: &mut ConnectionReader) -> io::Result<Vec<u8>> {
        let buffer = &mut connection_reader.buffer;
        connection_reader
            .reader
            .read_exact(&mut buffer[..2])
            .await?;
        let msg_len = u16::from_le_bytes(buffer[..2].try_into().unwrap()) as usize;
        connection_reader
            .reader
            .read_exact(&mut buffer[..msg_len])
            .await?;

        Ok(buffer[..msg_len].to_vec())
    }
}

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
                        if node.validate_message(&msg) {
                            if let Err(e) = node.process_message(msg, source) {
                                error!(parent: node.span(), "failed to handle an incoming message: {}", e);
                            }
                        } else {
                            error!(parent: node.span(), "failed to validate an incoming message");
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

    fn process_message(
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

    let mut generic_node_config = NodeConfig::default();
    generic_node_config.name = Some("generic".into());
    let generic_node = Node::new(Some(generic_node_config)).await.unwrap();
    let generic_node = Arc::new(GenericNode(generic_node));

    let mut echo_node_config = NodeConfig::default();
    echo_node_config.name = Some("echo".into());
    let echo_node = Arc::new(EchoNode {
        node: Node::new(Some(echo_node_config)).await.unwrap(),
        echoed: Default::default(),
    });

    generic_node.enable_reading_protocol();
    echo_node.enable_reading_protocol();
    echo_node.enable_response_protocol();

    generic_node
        .initiate_connection(echo_node.local_addr)
        .await
        .unwrap();

    generic_node
        .send_direct_message(echo_node.local_addr, vec![TestMessage::Herp as u8])
        .await
        .unwrap();
    generic_node
        .send_direct_message(echo_node.local_addr, vec![TestMessage::Derp as u8])
        .await
        .unwrap();
    generic_node
        .send_direct_message(echo_node.local_addr, vec![TestMessage::Herp as u8])
        .await
        .unwrap();

    sleep(Duration::from_millis(200)).await;
}
