mod common;

use tokio::time::sleep;
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

use pea2pea::{
    protocols::{Handshake, Reading, Writing},
    Config, Connection, ConnectionSide, Node, Pea2Pea,
};

use std::{io, net::SocketAddr, time::Duration};

#[derive(Clone)]
struct JoJoNode(Node);

impl Pea2Pea for JoJoNode {
    fn node(&self) -> &Node {
        &self.0
    }
}

#[async_trait::async_trait]
impl Handshake for JoJoNode {
    async fn perform_handshake(&self, conn: Connection) -> io::Result<Connection> {
        // some handshakes are useful, others are menacing ゴゴゴゴ
        match !conn.side {
            ConnectionSide::Initiator => {
                info!(parent: self.node().span(), "Dio!");
                sleep(Duration::from_secs(4)).await;
                info!(parent: self.node().span(), "I can't beat the shit out of you without getting closer.");
                sleep(Duration::from_secs(3)).await;
            }
            ConnectionSide::Responder => {
                sleep(Duration::from_secs(1)).await;
                warn!(parent: self.node().span(), "Oh, you're approaching me? Instead of running away, you're coming right to me?");
                sleep(Duration::from_secs(6)).await;
                warn!(parent: self.node().span(), "Oh ho! Then come as close as you like.");
            }
        }

        Ok(conn)
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum BattleCry {
    Ora = 0,
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

    fn read_message<R: io::Read>(
        &self,
        _source: SocketAddr,
        reader: &mut R,
    ) -> io::Result<Option<Self::Message>> {
        let mut arr = [0u8; 1];
        reader.read_exact(&mut arr)?;
        let battle_cry = BattleCry::from(arr[0]);

        Ok(Some(battle_cry))
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

        self.send_direct_message(source, reply)
            .unwrap()
            .await
            .unwrap();

        Ok(())
    }
}

impl Writing for JoJoNode {
    type Message = BattleCry;

    fn write_message<W: io::Write>(
        &self,
        _: SocketAddr,
        payload: &Self::Message,
        writer: &mut W,
    ) -> io::Result<()> {
        writer.write_all(&[*payload as u8])?;

        if *payload == BattleCry::Ora {
            info!(parent: self.node().span(), "{:?}!", payload);
        } else {
            warn!(parent: self.node().span(), "{:?}!", payload);
        };

        Ok(())
    }
}

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::INFO);

    let config = Config {
        name: Some("Jotaro".into()),
        max_handshake_time_ms: 10_000,
        ..Default::default()
    };
    let jotaro = JoJoNode(Node::new(Some(config)).await.unwrap());

    let config = Config {
        name: Some("Dio".into()),
        max_handshake_time_ms: 10_000,
        ..Default::default()
    };
    let dio = JoJoNode(Node::new(Some(config)).await.unwrap());
    let dio_addr = dio.node().listening_addr().unwrap();

    for node in &[&jotaro, &dio] {
        node.enable_handshake().await;
        node.enable_reading().await;
        node.enable_writing().await;
    }

    jotaro.node().connect(dio_addr).await.unwrap();

    sleep(Duration::from_secs(3)).await;

    jotaro
        .send_direct_message(dio_addr, BattleCry::Ora)
        .unwrap()
        .await
        .unwrap();

    sleep(Duration::from_secs(3)).await;
}
