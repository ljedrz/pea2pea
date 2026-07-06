//! A group of nodes playing the hot potato game.

use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicUsize, Ordering::Relaxed},
    },
    time::Duration,
};

use parking_lot::Mutex;
use pea2pea::{
    Config, Connection, ConnectionSide, Node, Pea2Pea, Topology, connect_nodes,
    protocols::{Handshake, Reading, Writing},
};
use rand::{RngExt, SeedableRng, rngs::SmallRng, seq::IteratorRandom};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::sleep,
};
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

static RNG: LazyLock<Mutex<SmallRng>> =
    LazyLock::new(|| Mutex::new(SmallRng::from_rng(&mut rand::rng())));

const NUM_PLAYERS: usize = 10;

type PlayerName = String;

// unset the previous potato carrier (if there was one)
fn clear_carrier(other_players: &mut HashMap<PlayerName, PlayerInfo>) {
    if let Some(old_carrier) = other_players.values_mut().find(|p| p.is_carrier) {
        old_carrier.is_carrier = false;
    }
    assert!(other_players.values().all(|p| !p.is_carrier));
}

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
        // a full mesh of loopback nodes needs per-IP connection headroom
        // (the default limit is 1)
        let config = Config {
            max_connections_per_ip: NUM_PLAYERS as u16,
            ..Default::default()
        };

        Self {
            node: Node::new(config),
            other_players: Default::default(),
            potato_count: Default::default(),
        }
    }

    async fn throw_potato(&self) {
        let message = Message::IHaveThePotato(self.node().name().into());
        let peers = self.node().connected_addrs();
        for addr in peers {
            let _ = self.unicast_fast(addr, message.clone());
        }

        let (new_carrier_name, new_carrier_addr) = self
            .other_players
            .lock()
            .iter()
            .map(|(name, player)| (name.clone(), player.addr))
            .choose(&mut *RNG.lock())
            .unwrap();

        info!(parent: self.node().span(), "throwing the potato to player {new_carrier_name}!");

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
                stream.write_all(self.node().name().as_bytes()).await?;

                // receive the peer's PlayerName
                let len = stream.read(&mut buffer).await?;

                String::from_utf8_lossy(&buffer[..len]).into_owned()
            }
            ConnectionSide::Responder => {
                // receive the peer's PlayerName
                let len = stream.read(&mut buffer).await?;
                let peer_name = String::from_utf8_lossy(&buffer[..len]).into_owned();

                // send own PlayerName
                stream.write_all(self.node().name().as_bytes()).await?;

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

#[derive(Clone, serde::Deserialize, serde::Serialize)]
enum Message {
    HotPotato,
    IHaveThePotato(PlayerName),
}

impl Reading for Player {
    type Message = Message;
    type Codec = examples::PostcardCodec<Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        // a single-byte length prefix suffices for these tiny messages
        examples::PostcardCodec::new(1)
    }

    async fn process_message(&self, _source: SocketAddr, message: Self::Message) {
        match message {
            Message::HotPotato => {
                info!(parent: self.node().span(), "I have the potato!");
                clear_carrier(&mut self.other_players.lock());

                self.potato_count.fetch_add(1, Relaxed);
                self.throw_potato().await;
            }
            Message::IHaveThePotato(carrier) => {
                let mut other_players = self.other_players.lock();

                clear_carrier(&mut other_players);
                if let Some(new_carrier) = other_players.get_mut(&carrier) {
                    new_carrier.is_carrier = true;
                }
            }
        }
    }
}

impl Writing for Player {
    type Message = Message;
    type Codec = examples::PostcardCodec<Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        // a single-byte length prefix suffices for these tiny messages
        examples::PostcardCodec::new(1)
    }
}

#[tokio::main]
async fn main() {
    // the game is very fast-paced; run with RUST_LOG=info to watch it live
    examples::start_logger(LevelFilter::OFF);

    const GAME_TIME_SECS: u64 = 5;

    println!("hot potato! players: {NUM_PLAYERS}, play time: {GAME_TIME_SECS}s");

    let players = (0..NUM_PLAYERS).map(|_| Player::new()).collect::<Vec<_>>();

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

    // nodes are never dropped implicitly; always shut them down once done
    for player in &players {
        player.node().shut_down().await;
    }
}
