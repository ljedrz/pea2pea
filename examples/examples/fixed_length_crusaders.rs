//! A JoJo-inspired example with fixed-length messages.

use std::{io, net::SocketAddr, time::Duration};

use bytes::{Buf, BufMut, BytesMut};
use pea2pea::{
    Config, Connection, ConnectionSide, Node, Pea2Pea,
    protocols::{Handshake, Reading, Writing},
};
use tokio::time::sleep;
use tokio_util::codec::{Decoder, Encoder};
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

#[derive(Clone)]
struct JoJoNode(Node);

impl Pea2Pea for JoJoNode {
    fn node(&self) -> &Node {
        &self.0
    }
}

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

impl TryFrom<u8> for BattleCry {
    type Error = io::Error;

    // the byte comes from the network, so it must not be trusted blindly
    fn try_from(byte: u8) -> io::Result<Self> {
        match byte {
            0 => Ok(BattleCry::Ora),
            1 => Ok(BattleCry::Muda),
            _ => Err(io::ErrorKind::InvalidData.into()),
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
        BattleCry::try_from(src.get_u8()).map(Some)
    }
}

impl Encoder<BattleCry> for SingleByteCodec {
    type Error = io::Error;

    fn encode(&mut self, item: BattleCry, dst: &mut BytesMut) -> Result<(), Self::Error> {
        dst.put_u8(item as u8);
        Ok(())
    }
}

impl Reading for JoJoNode {
    type Message = BattleCry;
    type Codec = SingleByteCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        SingleByteCodec
    }

    async fn process_message(&self, source: SocketAddr, battle_cry: Self::Message) {
        let reply = match battle_cry {
            BattleCry::Ora => BattleCry::Muda,
            BattleCry::Muda => BattleCry::Ora,
        };

        if reply == BattleCry::Ora {
            info!(parent: self.node().span(), "{:?}!", reply);
        } else {
            warn!(parent: self.node().span(), "{:?}!", reply);
        };

        if let Ok(rx) = self.unicast(source, reply) {
            let _ = rx.await;
        }
    }
}

impl Writing for JoJoNode {
    type Message = BattleCry;
    type Codec = SingleByteCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        SingleByteCodec
    }
}

#[tokio::main]
async fn main() {
    examples::start_logger(LevelFilter::INFO);

    let config = Config {
        name: Some("Jotaro".into()),
        ..Default::default()
    };
    let jotaro = JoJoNode(Node::new(config));

    let config = Config {
        name: Some("Dio".into()),
        ..Default::default()
    };
    let dio = JoJoNode(Node::new(config));
    let dio_addr = dio.node().toggle_listener().await.unwrap().unwrap();

    for node in &[&jotaro, &dio] {
        node.enable_handshake().await;
        node.enable_reading().await;
        node.enable_writing().await;
    }

    jotaro.node().connect(dio_addr).await.unwrap();

    sleep(Duration::from_secs(3)).await;

    let _ = jotaro.unicast(dio_addr, BattleCry::Ora).unwrap().await;

    sleep(Duration::from_secs(3)).await;

    // nodes are never dropped implicitly; always shut them down once done
    for node in [jotaro, dio] {
        node.node().shut_down().await;
    }
}
