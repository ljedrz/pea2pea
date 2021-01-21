mod common;

use bytes::Bytes;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use rand::{rngs::SmallRng, seq::IteratorRandom, Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
    time::sleep,
};
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

use pea2pea::{
    connect_nodes,
    protocols::{Handshaking, Reading, ReturnableConnection, Writing},
    ConnectionSide, Node, NodeConfig, Pea2Pea, Topology,
};

use std::{
    collections::HashMap,
    convert::TryInto,
    io,
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

static RNG: Lazy<Mutex<SmallRng>> = Lazy::new(|| Mutex::new(SmallRng::from_entropy()));

type PlayerName = String;

#[derive(Debug)]
struct PlayerInfo {
    name: PlayerName,
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
    async fn new(name: PlayerName) -> Self {
        let config = NodeConfig {
            name: Some(name),
            ..Default::default()
        };

        Self {
            node: Node::new(Some(config)).await.unwrap(),
            other_players: Default::default(),
            potato_count: Default::default(),
        }
    }

    async fn throw_potato(&self) {
        info!(parent: self.node().span(), "I have the potato!");
        let message = Message::IHaveThePotato(self.node().name().into());
        let message = bincode::serialize(&message).unwrap();
        self.node().send_broadcast(message.into()).await.unwrap();

        let (new_carrier_name, new_carrier_addr) = self
            .other_players
            .lock()
            .iter()
            .map(|(name, player)| (name.clone(), player.addr))
            .choose(&mut *RNG.lock())
            .unwrap();

        info!(parent: self.node().span(), "throwing the potato to {}!", new_carrier_name);

        let message = bincode::serialize(&Message::HotPotato).unwrap();
        self.node()
            .send_direct_message(new_carrier_addr, message.into())
            .await
            .unwrap();
    }
}

impl Pea2Pea for Player {
    fn node(&self) -> &Node {
        &self.node
    }
}

// prefixes the given message with its length
fn prefix_message(message: &[u8]) -> Bytes {
    let mut bytes = Vec::with_capacity(2 + message.len());
    bytes.extend_from_slice(&(message.len() as u16).to_le_bytes());
    bytes.extend_from_slice(message);
    bytes.into()
}

impl Handshaking for Player {
    fn enable_handshaking(&self) {
        let (from_node_sender, mut from_node_receiver) = mpsc::channel::<ReturnableConnection>(
            self.node().config().protocol_handler_queue_depth,
        );

        // spawn a background task dedicated to handling the handshakes
        let self_clone = self.clone();
        let handshaking_task = tokio::spawn(async move {
            loop {
                if let Some((mut conn, result_sender)) = from_node_receiver.recv().await {
                    let mut buffer = [0u8; 16];

                    let peer_name = match !conn.side {
                        ConnectionSide::Initiator => {
                            debug!(parent: conn.node.span(), "handshaking with {} as the initiator", conn.addr);

                            // send own PlayerName
                            let own_name = conn.node.name();
                            let message = prefix_message(own_name.as_bytes());
                            conn.writer().write_all(&message).await.unwrap();

                            // receive the peer's PlayerName
                            let len = conn.reader().read(&mut buffer).await.unwrap();

                            String::from_utf8(buffer[..len].to_vec()).unwrap()
                        }
                        ConnectionSide::Responder => {
                            debug!(parent: conn.node.span(), "handshaking with {} as the responder", conn.addr);

                            // receive the peer's PlayerName
                            let len = conn.reader().read(&mut buffer).await.unwrap();
                            let peer_name = String::from_utf8(buffer[..len].to_vec()).unwrap();

                            // send own PlayerName
                            let own_name = conn.node.name();
                            let message = prefix_message(own_name.as_bytes());
                            conn.writer().write_all(&message).await.unwrap();

                            peer_name
                        }
                    };

                    let player = PlayerInfo {
                        name: peer_name.clone(),
                        addr: conn.addr,
                        is_carrier: false,
                    };
                    self_clone.other_players.lock().insert(peer_name, player);

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

#[derive(Serialize, Deserialize)]
enum Message {
    HotPotato,
    IHaveThePotato(PlayerName),
}

#[async_trait::async_trait]
impl Reading for Player {
    type Message = Message;

    fn read_message(
        &self,
        _source: SocketAddr,
        buffer: &[u8],
    ) -> io::Result<Option<(Self::Message, usize)>> {
        // expecting incoming messages to be prefixed with their length encoded as a LE u16
        if buffer.len() >= 2 {
            let payload_len = u16::from_le_bytes(buffer[..2].try_into().unwrap()) as usize;

            if payload_len == 0 {
                return Err(io::ErrorKind::InvalidData.into());
            }

            if buffer[2..].len() >= payload_len {
                let message = bincode::deserialize(&buffer[2..][..payload_len]).unwrap();

                Ok(Some((message, 2 + payload_len)))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    async fn process_message(&self, _source: SocketAddr, message: Self::Message) -> io::Result<()> {
        match message {
            Message::HotPotato => {
                if let Some(ref mut old_carrier) = self
                    .other_players
                    .lock()
                    .values_mut()
                    .find(|p| p.is_carrier)
                {
                    old_carrier.is_carrier = false;
                }

                self.potato_count.fetch_add(1, Ordering::Relaxed);
                self.throw_potato().await;
            }
            Message::IHaveThePotato(carrier) => {
                let mut players = self.other_players.lock();

                if let Some(ref mut old_carrier) = players.values_mut().find(|p| p.is_carrier) {
                    old_carrier.is_carrier = false;
                }
                if let Some(ref mut new_carrier) = players.get_mut(&carrier) {
                    new_carrier.is_carrier = true;
                }
            }
        }

        Ok(())
    }
}

impl Writing for Player {
    fn write_message(&self, _: SocketAddr, payload: &[u8], buffer: &mut [u8]) -> io::Result<usize> {
        buffer[..2].copy_from_slice(&(payload.len() as u16).to_le_bytes());
        buffer[2..][..payload.len()].copy_from_slice(&payload);
        Ok(2 + payload.len())
    }
}

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::OFF);

    const GAME_TIME_SECS: u64 = 5;
    const NUM_PLAYERS: usize = 10;

    println!(
        "hot potato! players: {}, play time: {}s",
        NUM_PLAYERS, GAME_TIME_SECS
    );

    let mut players = Vec::with_capacity(NUM_PLAYERS);
    for i in 0..NUM_PLAYERS {
        players.push(Player::new(format!("player {}", i)).await);
    }

    for player in &players {
        player.enable_handshaking();
        player.enable_reading();
        player.enable_writing();
    }
    connect_nodes(&players, Topology::Mesh).await.unwrap();

    let first_carrier = RNG.lock().gen_range(0..NUM_PLAYERS);
    players[first_carrier]
        .potato_count
        .fetch_add(1, Ordering::Relaxed);
    players[first_carrier].throw_potato().await;

    sleep(Duration::from_secs(GAME_TIME_SECS)).await;

    println!("\n---------- scoreboard ----------");
    for player in &players {
        println!(
            "{} got the potato {} times",
            player.node().name(),
            player.potato_count.load(Ordering::Relaxed)
        );
    }
}
