use parking_lot::Mutex;
use tokio::{sync::mpsc::channel, time::sleep};
use tracing::*;

use pea2pea::{Node, ResponseProtocol};

use std::{collections::HashSet, io, net::SocketAddr, sync::Arc, time::Duration};

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

impl std::ops::Deref for EchoNode {
    type Target = Node;

    fn deref(&self) -> &Self::Target {
        &self.node
    }
}

impl ResponseProtocol for EchoNode {
    type Message = TestMessage;

    fn enable_response_protocol(self: &Arc<Self>) {
        let (sender, mut receiver) = channel(4);
        self.node.incoming_requests.set(Some(sender)).unwrap();

        let node = Arc::clone(&self);
        tokio::spawn(async move {
            loop {
                if let Some((request, source)) = receiver.recv().await {
                    if let Some(msg) = node.parse_message(&request) {
                        if node.validate_message(&msg) {
                            if let Err(e) = node.process_message(msg, source) {
                                error!("failed to handle an incoming message: {}", e);
                            }
                        } else {
                            error!("failed to validate an incoming message");
                        }
                    } else {
                        error!("can't parse an incoming message");
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
        info!("got a {:?} from {}", message, source_addr);
        if self.echoed.lock().insert(message) {
            info!("it was new! echoing it");

            let node = Arc::clone(self);
            tokio::spawn(async move {
                node.send_direct_message(source_addr, vec![message as u8])
                    .await
                    .unwrap();
            });
        } else {
            debug!("I've already seen {:?}! not echoing", message);
        }

        Ok(())
    }
}

#[tokio::test]
async fn request_handling_echo() {
    tracing_subscriber::fmt::init();

    let generic_node = Node::new(None).await.unwrap();

    let echo_node = Arc::new(EchoNode {
        node: Node::new(None).await.unwrap(),
        echoed: Default::default(),
    });
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
