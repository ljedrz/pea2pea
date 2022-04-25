mod common;

use bytes::{Buf, BufMut, BytesMut};
use tokio::time::sleep;
use tokio_util::codec::{Decoder, Encoder};
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
    const TIMEOUT_MS: u64 = 10_000;

    async fn perform_handshake(&self, conn: Connection) -> io::Result<Connection> {
        // some handshakes are useful, others are menacing ゴゴゴゴ
        match !conn.side() {
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

struct SingleByteCodec;

impl Decoder for SingleByteCodec {
    type Item = BattleCry;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.is_empty() {
            return Ok(None);
        }
        Ok(Some(src.get_u8().into()))
    }
}

impl Encoder<BattleCry> for SingleByteCodec {
    type Error = io::Error;

    fn encode(&mut self, item: BattleCry, dst: &mut BytesMut) -> Result<(), Self::Error> {
        dst.put_u8(item as u8);
        Ok(())
    }
}

#[async_trait::async_trait]
impl Reading for JoJoNode {
    type Message = BattleCry;
    type Codec = SingleByteCodec;

    fn codec(&self, _addr: SocketAddr) -> Self::Codec {
        SingleByteCodec
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

        if reply == BattleCry::Ora {
            info!(parent: self.node().span(), "{:?}!", reply);
        } else {
            warn!(parent: self.node().span(), "{:?}!", reply);
        };

        let _ = self.send_direct_message(source, reply).unwrap().await;

        Ok(())
    }
}

impl Writing for JoJoNode {
    type Message = BattleCry;
    type Codec = SingleByteCodec;

    fn codec(&self, _addr: SocketAddr) -> Self::Codec {
        SingleByteCodec
    }
}

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::INFO);

    let config = Config {
        name: Some("Jotaro".into()),
        ..Default::default()
    };
    let jotaro = JoJoNode(Node::new(Some(config)).await.unwrap());

    let config = Config {
        name: Some("Dio".into()),
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

    let _ = jotaro
        .send_direct_message(dio_addr, BattleCry::Ora)
        .unwrap()
        .await;

    sleep(Duration::from_secs(3)).await;
}
