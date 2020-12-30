use bytes::Bytes;
use parking_lot::Mutex;
use rand::{rngs::SmallRng, seq::IteratorRandom, Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use tokio::{sync::mpsc, time::sleep};
use tracing::*;

use pea2pea::{
    connect_nodes, ConnectionSide, HandshakeObjects, Handshaking, Messaging, Node, NodeConfig,
    Pea2Pea, Topology,
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
        let (from_node_sender, mut from_node_receiver) = mpsc::channel::<HandshakeObjects>(1);

        // spawn a background task dedicated to handling the handshakes
        let self_clone = self.clone();
        tokio::spawn(async move {
            loop {
                if let Some((mut conn_reader, conn, result_sender)) =
                    from_node_receiver.recv().await
                {
                    let node = Arc::clone(&conn_reader.node);
                    let addr = conn_reader.addr;

                    let peer_name = match conn.side {
                        // the connection is the Responder, so the node is the Initiator
                        ConnectionSide::Responder => {
                            debug!(parent: node.span(), "handshaking with {} as the initiator", addr);

                            // send own PlayerName
                            let own_name = node.name();
                            let message = prefix_message(own_name.as_bytes());
                            conn.send_message(message).await;

                            // receive the peer's PlayerName
                            let message = conn_reader.read_queued_bytes().await.unwrap();

                            String::from_utf8(message[2..].to_vec()).unwrap()
                        }
                        // the connection is the Initiator, so the node is the Responder
                        ConnectionSide::Initiator => {
                            debug!(parent: node.span(), "handshaking with {} as the responder", addr);

                            // receive the peer's PlayerName
                            let message = conn_reader.read_queued_bytes().await.unwrap();
                            let peer_name = String::from_utf8(message[2..].to_vec()).unwrap();

                            // send own PlayerName
                            let own_name = node.name();
                            let message = prefix_message(own_name.as_bytes());
                            conn.send_message(message).await;

                            peer_name
                        }
                    };

                    let player = Player {
                        name: peer_name.clone(),
                        addr,
                        is_carrier: false,
                    };
                    self_clone.other_players.lock().insert(peer_name, player);

                    // return the connection objects to the node
                    if result_sender.send(Ok((conn_reader, conn))).is_err() {
                        // can't recover if this happens
                        unreachable!();
                    }
                }
            }
        });

        self.node().set_handshake_handler(from_node_sender.into());
    }
}

#[derive(Serialize, Deserialize)]
enum Message {
    HotPotato,
    IHaveThePotato(PlayerName),
}

#[async_trait::async_trait]
impl Messaging for PlayerNode {
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
                let message = bincode::deserialize(&buffer[2..2 + payload_len]).unwrap();

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
