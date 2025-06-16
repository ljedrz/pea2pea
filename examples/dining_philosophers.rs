//! A P2P rendition of the dining philosophers problem.

mod common;

use std::{
    io, mem,
    net::SocketAddr,
    sync::{Arc, LazyLock, OnceLock},
    time::Duration,
};

use bincode::{Decode, Encode};
use bytes::BytesMut;
use pea2pea::{
    connect_nodes,
    protocols::{Reading, Writing},
    Config, ConnectionSide, Node, Pea2Pea, Topology,
};
use rand::{rngs::SmallRng, Rng, SeedableRng};
use tokio::{sync::RwLock, time::sleep};
use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec};
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

static RNG: LazyLock<parking_lot::Mutex<SmallRng>> =
    LazyLock::new(|| parking_lot::Mutex::new(SmallRng::from_os_rng()));

const MIN_EATING_TIME_MS: u64 = 500;
const MAX_EATING_TIME_MS: u64 = 1000;

const MIN_THINKING_TIME_MS: u64 = 2000;
const MAX_THINKING_TIME_MS: u64 = 5000;

#[derive(Clone)]
struct Philosopher {
    node: Node,
    state: Arc<RwLock<State>>,
    left_neighbor: Arc<OnceLock<(SocketAddr, String)>>,
    right_neighbor: Arc<OnceLock<(SocketAddr, String)>>,
}

impl Pea2Pea for Philosopher {
    fn node(&self) -> &Node {
        &self.node
    }
}

#[derive(Debug, PartialEq, Clone)]
enum State {
    Thinking,
    Hungry(bool), // has the left fork yet?
    Eating(Duration),
}

#[derive(Clone, Copy, Debug, PartialEq, Decode, Encode)]
enum Message {
    AreYouUsingTheSharedFork,
    Yes(Option<Duration>), // eating duration (if the responder is eating)
    No,
}

impl Philosopher {
    async fn new(name: String) -> Self {
        let config = Config {
            name: Some(name),
            ..Default::default()
        };

        let node = Philosopher {
            node: Node::new(config),
            state: Arc::new(RwLock::new(State::Thinking)),
            left_neighbor: Default::default(),
            right_neighbor: Default::default(),
        };

        node.enable_reading().await;
        node.enable_writing().await;
        node.node().toggle_listener().await.unwrap();

        node
    }

    fn start_dining(&self) {
        let node = self.clone();
        tokio::spawn(async move {
            loop {
                let state = node.state.read().await;
                match (*state).clone() {
                    State::Thinking => {
                        info!(parent: node.node().span(), "I'm thinking");

                        let thinking_time = RNG
                            .lock()
                            .random_range(MIN_THINKING_TIME_MS..=MAX_THINKING_TIME_MS);
                        sleep(Duration::from_millis(thinking_time)).await;
                        drop(state);
                        *node.state.write().await = State::Hungry(false);
                        info!(parent: node.node().span(), "I'm hungry");
                    }
                    State::Hungry(false) => {
                        let left_neighbor = node.left_neighbor.get().unwrap();
                        debug!(parent: node.node().span(), "asking {} for the fork", left_neighbor.1);
                        drop(state);
                        node.unicast(left_neighbor.0, Message::AreYouUsingTheSharedFork)
                            .unwrap();
                        sleep(Duration::from_millis(250)).await;
                    }
                    State::Hungry(true) => {
                        let right_neighbor = node.right_neighbor.get().unwrap();
                        debug!(parent: node.node().span(), "asking {} for the fork", right_neighbor.1);
                        drop(state);
                        node.unicast(right_neighbor.0, Message::AreYouUsingTheSharedFork)
                            .unwrap();
                        sleep(Duration::from_millis(250)).await;
                    }
                    State::Eating(duration) => {
                        info!(parent: node.node().span(), "I'm eating");

                        sleep(duration).await;
                        drop(state);
                        *node.state.write().await = State::Thinking;
                    }
                }
            }
        });
    }
}

struct Codec(LengthDelimitedCodec);

impl Default for Codec {
    fn default() -> Self {
        let inner = LengthDelimitedCodec::builder()
            .length_field_length(1)
            .new_codec();
        Self(inner)
    }
}

impl Decoder for Codec {
    type Item = Message;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let bytes = self.0.decode(src)?;
        if bytes.is_none() {
            return Ok(None);
        }

        let message = bincode::decode_from_slice(&bytes.unwrap(), bincode::config::standard())
            .map_err(|_| io::ErrorKind::InvalidData)?
            .0;

        Ok(Some(message))
    }
}

impl Encoder<Message> for Codec {
    type Error = io::Error;

    fn encode(&mut self, item: Message, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let bytes = bincode::encode_to_vec(item, bincode::config::standard())
            .unwrap()
            .into();
        self.0.encode(bytes, dst)
    }
}

impl Reading for Philosopher {
    type Message = Message;
    type Codec = Codec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()> {
        let (left_neighbor_addr, left_neighbor_name) = self.left_neighbor.get().unwrap();
        let (neighbor_name, neighbor_side) = if source == *left_neighbor_addr {
            (left_neighbor_name.to_owned(), "left")
        } else {
            (self.right_neighbor.get().unwrap().1.to_owned(), "right")
        };

        match message {
            Message::AreYouUsingTheSharedFork => {
                let answer = if matches!(*self.state.read().await, State::Thinking) {
                    debug!(parent: self.node().span(), "giving {} my {} fork", neighbor_name, neighbor_side);
                    Message::No
                } else {
                    debug!(parent: self.node().span(), "I'm not giving {} my {} fork yet", neighbor_name, neighbor_side);
                    Message::Yes(None)
                };

                self.unicast(source, answer)
                    .unwrap()
                    .await
                    .unwrap()
                    .unwrap();
            }
            Message::Yes(duration) => {
                debug!(parent: self.node().span(), "{} won't share his fork yet", neighbor_name);

                let state = self.state.read().await;
                if *state != State::Hungry(true) {
                    drop(state);
                    *self.state.write().await = State::Thinking;
                } else if let Some(time) = duration {
                    sleep(time).await;
                }
            }
            Message::No => {
                info!(parent: self.node().span(), "I got the fork from {}", neighbor_name);

                let state = &mut *self.state.write().await;
                if *state == State::Hungry(false) {
                    *state = State::Hungry(true);
                } else if *state == State::Hungry(true) {
                    let eating_time = RNG
                        .lock()
                        .random_range(MIN_EATING_TIME_MS..=MAX_EATING_TIME_MS);
                    *state = State::Eating(Duration::from_millis(eating_time));
                }
            }
        }

        Ok(())
    }
}

impl Writing for Philosopher {
    type Message = Message;
    type Codec = Codec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    common::start_logger(LevelFilter::INFO);

    let philosophers = vec![
        Philosopher::new("Socrates".to_owned()).await,
        Philosopher::new("Diogenes".to_owned()).await,
        Philosopher::new("Kant".to_owned()).await,
        Philosopher::new("Nietzsche".to_owned()).await,
        Philosopher::new("Wittgenstein".to_owned()).await,
    ];

    connect_nodes(&philosophers, Topology::Ring).await.unwrap();
    sleep(Duration::from_millis(100)).await;

    let mut previous_node_name = philosophers.last().unwrap().node().name().to_owned();
    for (p1, p2) in philosophers.iter().zip(
        philosophers
            .iter()
            .skip(1)
            .chain(philosophers.iter().next()),
    ) {
        let right_neighbor_addr = p2.node().listening_addr().await.unwrap();
        let right_neighbor_name = p2.node().name().to_owned();

        let both_neighbors = p1.node().connected_addrs();

        assert_eq!(both_neighbors.len(), 2);

        let left_neighbor_addr = both_neighbors
            .into_iter()
            .find(|addr| *addr != right_neighbor_addr)
            .unwrap();
        let left_neighbor_name = mem::replace(&mut previous_node_name, p1.node().name().to_owned());

        p1.right_neighbor
            .set((right_neighbor_addr, right_neighbor_name))
            .unwrap();
        p1.left_neighbor
            .set((left_neighbor_addr, left_neighbor_name))
            .unwrap();
    }

    for p in &philosophers {
        p.start_dining();
    }

    sleep(Duration::from_secs(60)).await;
}
