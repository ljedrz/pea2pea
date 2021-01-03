use bytes::Bytes;
use tokio::time::sleep;
use tracing::*;

mod common;
use pea2pea::{
    protocols::{Reading, Writing},
    Node, NodeConfig, Pea2Pea,
};

use std::{future, io, net::SocketAddr, sync::Arc, time::Duration};

#[derive(Clone)]
struct JoJoNode(Arc<Node>);

impl Pea2Pea for JoJoNode {
    fn node(&self) -> &Arc<Node> {
        &self.0
    }
}

#[derive(Debug)]
enum BattleCry {
    Ora,
    Muda,
}

#[async_trait::async_trait]
impl Reading for JoJoNode {
    type Message = BattleCry;

    fn read_message(
        &self,
        _source: SocketAddr,
        buffer: &[u8],
    ) -> io::Result<Option<(Self::Message, usize)>> {
        let battle_cry = match buffer[0] as u8 {
            0 => BattleCry::Ora,
            1 => BattleCry::Muda,
            _ => unreachable!(),
        };

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

        match buffer[0] {
            0 => {
                info!(parent: self.node().span(), "{:?}!", BattleCry::Ora);
            }
            1 => {
                warn!(parent: self.node().span(), "{:?}!", BattleCry::Muda);
            }
            _ => unreachable!(),
        };

        Ok(1)
    }
}

// this one can be considered a stress-test
#[tokio::test]
#[ignore]
async fn fixed_length_crusaders() {
    tracing_subscriber::fmt::init();

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
        node.enable_reading();
        node.enable_writing();
    }

    sleep(Duration::from_secs(1)).await;

    info!("Jotaro: Dio!");
    sleep(Duration::from_secs(1)).await;

    warn!("Dio: Oh, you're approaching me? Instead of running away, you're coming right to me?");
    sleep(Duration::from_secs(1)).await;

    info!("Jotaro: I can't beat the shit out of you without getting closer.");
    sleep(Duration::from_secs(1)).await;

    warn!("Dio: Oh ho! Then come as close as you like.");
    sleep(Duration::from_secs(3)).await;

    jotaro
        .node()
        .connect(dio.node().listening_addr)
        .await
        .unwrap();

    jotaro
        .node()
        .send_direct_message(
            dio.node().listening_addr,
            Bytes::copy_from_slice(&[BattleCry::Ora as u8]),
        )
        .await
        .unwrap();

    future::pending::<()>().await;
}
