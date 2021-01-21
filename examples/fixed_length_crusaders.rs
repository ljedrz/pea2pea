mod common;

use bytes::Bytes;
use tokio::{sync::mpsc, time::sleep};
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

use pea2pea::{
    connections::ConnectionSide,
    protocols::{Handshaking, Reading, ReturnableConnection, Writing},
    Node, NodeConfig, Pea2Pea,
};

use std::{io, net::SocketAddr, time::Duration};

#[derive(Clone)]
struct JoJoNode(Node);

impl Pea2Pea for JoJoNode {
    fn node(&self) -> &Node {
        &self.0
    }
}

impl Handshaking for JoJoNode {
    fn enable_handshaking(&self) {
        let (from_node_sender, mut from_node_receiver) = mpsc::channel::<ReturnableConnection>(
            self.node().config().protocol_handler_queue_depth,
        );

        // spawn a background task dedicated to handling the handshakes
        let handshaking_task = tokio::spawn(async move {
            loop {
                if let Some((conn, result_sender)) = from_node_receiver.recv().await {
                    // some handshakes are useful, others are menacing ゴゴゴゴ
                    match !conn.side {
                        ConnectionSide::Initiator => {
                            info!(parent: conn.node.span(), "Dio!");
                            sleep(Duration::from_secs(4)).await;
                            info!(parent: conn.node.span(), "I can't beat the shit out of you without getting closer.");
                            sleep(Duration::from_secs(3)).await;
                        }
                        ConnectionSide::Responder => {
                            sleep(Duration::from_secs(1)).await;
                            warn!(parent: conn.node.span(), "Oh, you're approaching me? Instead of running away, you're coming right to me?");
                            sleep(Duration::from_secs(6)).await;
                            warn!(parent: conn.node.span(), "Oh ho! Then come as close as you like.");
                        }
                    };

                    // return the Connection to the node
                    if result_sender.send(Ok(conn)).is_err() {
                        unreachable!(); // can't recover if this happens
                    }
                }
            }
        });

        self.node()
            .set_handshake_handler((from_node_sender, handshaking_task).into());
    }
}

#[derive(Debug, PartialEq)]
enum BattleCry {
    Ora,
    Muda,
}

impl From<u8> for BattleCry {
    fn from(byte: u8) -> Self {
        match byte {
            0 => BattleCry::Ora,
            1 => BattleCry::Muda,
            _ => unreachable!(),
        }
    }
}

#[async_trait::async_trait]
impl Reading for JoJoNode {
    type Message = BattleCry;

    fn read_message(
        &self,
        _source: SocketAddr,
        buffer: &[u8],
    ) -> io::Result<Option<(Self::Message, usize)>> {
        let battle_cry = BattleCry::from(buffer[0]);

        Ok(Some((battle_cry, 1)))
    }

    async fn process_message(
        &self,
        source: SocketAddr,
        battle_cry: Self::Message,
    ) -> io::Result<()> {
        let reply = match battle_cry {
            BattleCry::Ora => BattleCry::Muda,
            BattleCry::Muda => BattleCry::Ora,
        };

        self.node()
            .send_direct_message(source, Bytes::copy_from_slice(&[reply as u8]))
            .await
    }
}

impl Writing for JoJoNode {
    fn write_message(&self, _: SocketAddr, payload: &[u8], buffer: &mut [u8]) -> io::Result<usize> {
        buffer[0] = payload[0];
        let battle_cry = BattleCry::from(buffer[0]);

        if battle_cry == BattleCry::Ora {
            info!(parent: self.node().span(), "{:?}!", battle_cry);
        } else {
            warn!(parent: self.node().span(), "{:?}!", battle_cry);
        };

        Ok(1)
    }
}

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::INFO);

    let config = NodeConfig {
        name: Some("Jotaro".into()),
        ..Default::default()
    };
    let jotaro = JoJoNode(Node::new(Some(config)).await.unwrap());

    let config = NodeConfig {
        name: Some("Dio".into()),
        ..Default::default()
    };
    let dio = JoJoNode(Node::new(Some(config)).await.unwrap());

    for node in &[&jotaro, &dio] {
        node.enable_handshaking();
        node.enable_reading();
        node.enable_writing();
    }

    jotaro
        .node()
        .connect(dio.node().listening_addr())
        .await
        .unwrap();

    sleep(Duration::from_secs(3)).await;

    jotaro
        .node()
        .send_direct_message(
            dio.node().listening_addr(),
            Bytes::copy_from_slice(&[BattleCry::Ora as u8]),
        )
        .await
        .unwrap();

    sleep(Duration::from_secs(5)).await;
}
