use bytes::Bytes;
use parking_lot::Mutex;
use rand::{rngs::SmallRng, seq::IteratorRandom, Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use tokio::{sync::mpsc::channel, task::JoinHandle, time::sleep};
use tracing::*;

use pea2pea::{
    connect_nodes, Connection, ConnectionReader, HandshakeSetup, HandshakeState, Handshaking,
    Messaging, Node, NodeConfig, Pea2Pea, Topology,
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

type PlayerName = String;

#[derive(Debug)]
struct Player {
    name: PlayerName,
    addr: SocketAddr,
    is_carrier: bool,
}

#[derive(Clone)]
struct PlayerNode {
    node: Arc<Node>,
    other_players: Arc<Mutex<HashMap<PlayerName, Player>>>,
    rng: Arc<Mutex<SmallRng>>,
    potato_count: Arc<AtomicUsize>,
}

impl PlayerNode {
    async fn new(name: PlayerName, rng: Arc<Mutex<SmallRng>>) -> Self {
        let mut config = NodeConfig::default();
        config.name = Some(name);
        let node = Node::new(Some(config)).await.unwrap();

        Self {
            node,
            other_players: Default::default(),
            rng,
            potato_count: Default::default(),
        }
    }

    async fn throw_potato(&self) {
        info!(parent: self.node().span(), "I have the potato!");
        let message = Message::IHaveThePotato(self.node().name().into());
        let message = bincode::serialize(&message).unwrap();
        self.node().send_broadcast(prefix_message(&message)).await;

        let (new_carrier_name, new_carrier_addr) = self
            .other_players
            .lock()
            .iter()
            .map(|(name, player)| (name.clone(), player.addr))
            .choose(&mut *self.rng.lock())
            .unwrap();

        info!(parent: self.node().span(), "throwing the potato to {}!", new_carrier_name);

        let message = bincode::serialize(&Message::HotPotato).unwrap();
        self.node()
            .send_direct_message(new_carrier_addr, prefix_message(&message))
            .await
            .unwrap();
    }
}

impl Pea2Pea for PlayerNode {
    fn node(&self) -> &Arc<Node> {
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

impl Handshaking for PlayerNode {
    fn enable_handshaking(&self) {
        let (state_sender, mut state_receiver) = channel::<(SocketAddr, HandshakeState)>(64);

        let self_clone = self.clone();
        tokio::spawn(async move {
            loop {
                if let Some((addr, state)) = state_receiver.recv().await {
                    let name: String = *state.downcast().unwrap();
                    let player = Player {
                        name: name.clone(),
                        addr,
                        is_carrier: false,
                    };
                    self_clone.other_players.lock().insert(name, player);
                }
            }
        });

        let initiator =
            |mut connection_reader: ConnectionReader,
             connection: Connection|
             -> JoinHandle<io::Result<(ConnectionReader, Connection, HandshakeState)>> {
                tokio::spawn(async move {
                    let node = Arc::clone(&connection_reader.node);
                    let addr = connection_reader.addr;
                    debug!(parent: node.span(), "handshaking with {} as the initiator", addr);

                    // send own PlayerName
                    let own_name = node.name();
                    let message = prefix_message(own_name.as_bytes());
                    connection.send_message(message).await;

                    // receive the peer's PlayerName
                    let message = connection_reader.read_queued_bytes().await.unwrap();
                    let peer_name = String::from_utf8(message[2..].to_vec()).unwrap();

                    Ok((
                        connection_reader,
                        connection,
                        Box::new(peer_name) as HandshakeState,
                    ))
                })
            };

        let responder =
            |mut connection_reader: ConnectionReader,
             connection: Connection|
             -> JoinHandle<io::Result<(ConnectionReader, Connection, HandshakeState)>> {
                tokio::spawn(async move {
                    let node = Arc::clone(&connection_reader.node);
                    let addr = connection_reader.addr;
                    debug!(parent: node.span(), "handshaking with {} as the responder", addr);

                    // receive the peer's PlayerName
                    let message = connection_reader.read_queued_bytes().await.unwrap();
                    let peer_name = String::from_utf8(message[2..].to_vec()).unwrap();

                    // send own PlayerName
                    let own_name = node.name();
                    let message = prefix_message(own_name.as_bytes());
                    connection.send_message(message).await;

                    Ok((
                        connection_reader,
                        connection,
                        Box::new(peer_name) as HandshakeState,
                    ))
                })
            };

        let handshake_setup = HandshakeSetup {
            initiator_closure: Box::new(initiator),
            responder_closure: Box::new(responder),
            state_sender: Some(state_sender),
        };

        self.node().set_handshake_setup(handshake_setup);
    }
}

#[derive(Serialize, Deserialize)]
enum Message {
    HotPotato,
    IHaveThePotato(PlayerName),
}

#[async_trait::async_trait]
impl Messaging for PlayerNode {
    fn read_message(buffer: &[u8]) -> io::Result<Option<&[u8]>> {
        // expecting incoming messages to be prefixed with their length encoded as a LE u16
        if buffer.len() >= 2 {
            let payload_len = u16::from_le_bytes(buffer[..2].try_into().unwrap()) as usize;

            if payload_len == 0 {
                return Err(io::ErrorKind::InvalidData.into());
            }

            if buffer[2..].len() >= payload_len {
                Ok(Some(&buffer[..2 + payload_len]))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    async fn process_message(&self, _source: SocketAddr, message: Bytes) -> io::Result<()> {
        let message = bincode::deserialize(&message[2..]).unwrap();

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

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    const GAME_TIME_SECS: u64 = 5;
    const NUM_PLAYERS: usize = 10;

    println!(
        "hot potato! players: {}, play time: {}s",
        NUM_PLAYERS, GAME_TIME_SECS
    );

    let rng = Arc::new(Mutex::new(SmallRng::from_entropy()));

    let mut players = Vec::with_capacity(NUM_PLAYERS);
    for i in 0..NUM_PLAYERS {
        players.push(PlayerNode::new(format!("player {}", i), rng.clone()).await);
    }

    for player in &players {
        player.enable_handshaking();
        player.enable_messaging();
    }
    connect_nodes(&players, Topology::Mesh).await.unwrap();

    let first_carrier = rng.lock().gen_range(0..NUM_PLAYERS);
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
