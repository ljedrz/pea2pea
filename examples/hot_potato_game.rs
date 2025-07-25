//! A group of nodes playing the hot potato game.

mod common;

use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering::Relaxed},
        Arc, LazyLock,
    },
    time::Duration,
};

use bincode::{Decode, Encode};
use bytes::BytesMut;
use parking_lot::Mutex;
use pea2pea::{
    connect_nodes,
    protocols::{Handshake, Reading, Writing},
    Connection, ConnectionSide, Node, Pea2Pea, Topology,
};
use rand::{rngs::SmallRng, seq::IteratorRandom, Rng, SeedableRng};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::sleep,
};
use tokio_util::codec::{Decoder, Encoder};
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

static RNG: LazyLock<Mutex<SmallRng>> = LazyLock::new(|| Mutex::new(SmallRng::from_os_rng()));

type PlayerName = String;

#[derive(Debug)]
struct PlayerInfo {
    addr: SocketAddr,
    is_carrier: bool,
}

#[derive(Clone)]
struct Player {
    node: Node,
    other_players: Arc<Mutex<HashMap<PlayerName, PlayerInfo>>>,
    potato_count: Arc<AtomicUsize>,
}

impl Player {
    fn new() -> Self {
        Self {
            node: Node::new(Default::default()),
            other_players: Default::default(),
            potato_count: Default::default(),
        }
    }

    async fn throw_potato(&self) {
        let message = Message::IHaveThePotato(self.node().name().into());
        self.broadcast(message).unwrap();

        let (new_carrier_name, new_carrier_addr) = self
            .other_players
            .lock()
            .iter()
            .map(|(name, player)| (name.clone(), player.addr))
            .choose(&mut *RNG.lock())
            .unwrap();

        info!(parent: self.node().span(), "throwing the potato to player {}!", new_carrier_name);

        let _ = self
            .unicast(new_carrier_addr, Message::HotPotato)
            .unwrap()
            .await;
    }
}

impl Pea2Pea for Player {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Handshake for Player {
    async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
        let mut buffer = [0u8; 16];

        let node_conn_side = !conn.side();
        let stream = self.borrow_stream(&mut conn);

        let peer_name = match node_conn_side {
            ConnectionSide::Initiator => {
                // send own PlayerName
                let own_name = self.node().name().as_bytes().to_vec();
                stream.write_all(&own_name).await?;

                // receive the peer's PlayerName
                let len = stream.read(&mut buffer).await?;

                String::from_utf8_lossy(&buffer[..len]).into_owned()
            }
            ConnectionSide::Responder => {
                // receive the peer's PlayerName
                let len = stream.read(&mut buffer).await?;
                let peer_name = String::from_utf8_lossy(&buffer[..len]).into_owned();

                // send own PlayerName
                let own_name = self.node().name().as_bytes().to_vec();
                stream.write_all(&own_name).await?;

                peer_name
            }
        };

        let player = PlayerInfo {
            addr: conn.addr(),
            is_carrier: false,
        };
        self.other_players.lock().insert(peer_name, player);

        Ok(conn)
    }
}

#[derive(Clone, Decode, Encode)]
enum Message {
    HotPotato,
    IHaveThePotato(PlayerName),
}

impl Decoder for common::TestCodec<Message> {
    type Item = Message;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.0
            .decode(src)?
            .map(|b| {
                bincode::borrow_decode_from_slice(&b, bincode::config::standard())
                    .map_err(|_| io::ErrorKind::InvalidData.into())
            })
            .transpose()
            .map(|r| r.map(|t| t.0))
    }
}

impl Reading for Player {
    type Message = Message;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, _source: SocketAddr, message: Self::Message) -> io::Result<()> {
        match message {
            Message::HotPotato => {
                info!(parent: self.node().span(), "I have the potato!");
                {
                    let mut other_players = self.other_players.lock();
                    if let Some(old_carrier) = other_players.values_mut().find(|p| p.is_carrier) {
                        old_carrier.is_carrier = false;
                    }
                    assert!(other_players.values().all(|p| !p.is_carrier));
                }

                self.potato_count.fetch_add(1, Relaxed);
                self.throw_potato().await;
            }
            Message::IHaveThePotato(carrier) => {
                let mut other_players = self.other_players.lock();

                if let Some(old_carrier) = other_players.values_mut().find(|p| p.is_carrier) {
                    old_carrier.is_carrier = false;
                }
                assert!(other_players.values().all(|p| !p.is_carrier));
                if let Some(new_carrier) = other_players.get_mut(&carrier) {
                    new_carrier.is_carrier = true;
                }
            }
        }

        Ok(())
    }
}

impl<M> Encoder<Message> for common::TestCodec<M> {
    type Error = io::Error;

    fn encode(&mut self, item: Message, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let bytes = bincode::encode_to_vec(&item, bincode::config::standard())
            .unwrap()
            .into();
        self.0.encode(bytes, dst)
    }
}

impl Writing for Player {
    type Message = Message;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::OFF);

    const GAME_TIME_SECS: u64 = 5;
    const NUM_PLAYERS: usize = 10;

    println!("hot potato! players: {NUM_PLAYERS}, play time: {GAME_TIME_SECS}s");

    let mut players = Vec::with_capacity(NUM_PLAYERS);
    for _ in 0..NUM_PLAYERS {
        players.push(Player::new());
    }

    for player in &players {
        player.enable_handshake().await;
        player.enable_reading().await;
        player.enable_writing().await;
        player.node().toggle_listener().await.unwrap();
    }
    connect_nodes(&players, Topology::Mesh).await.unwrap();

    let first_carrier = RNG.lock().random_range(0..NUM_PLAYERS);
    players[first_carrier].potato_count.fetch_add(1, Relaxed);
    players[first_carrier].throw_potato().await;

    sleep(Duration::from_secs(GAME_TIME_SECS)).await;

    println!("\n---------- scoreboard ----------");
    for player in &players {
        println!(
            "player {} got the potato {} times",
            player.node().name(),
            player.potato_count.load(Relaxed)
        );
    }
}
