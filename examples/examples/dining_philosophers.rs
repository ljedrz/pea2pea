//! A P2P rendition of the dining philosophers problem.

use std::{
    net::SocketAddr,
    sync::{Arc, LazyLock, OnceLock},
    time::Duration,
};

use pea2pea::{
    Config, ConnectionSide, Node, Pea2Pea, Topology, connect_nodes,
    protocols::{Reading, Writing},
};
use rand::{RngExt, SeedableRng, rngs::SmallRng};
use tokio::{sync::RwLock, time::sleep};
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

static RNG: LazyLock<parking_lot::Mutex<SmallRng>> =
    LazyLock::new(|| parking_lot::Mutex::new(SmallRng::from_rng(&mut rand::rng())));

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

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
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
                    State::Hungry(has_left_fork) => {
                        // ask for the left fork first, then the right one
                        let neighbor = if has_left_fork {
                            node.right_neighbor.get().unwrap()
                        } else {
                            node.left_neighbor.get().unwrap()
                        };
                        debug!(parent: node.node().span(), "asking {} for the fork", neighbor.1);
                        drop(state);
                        node.unicast_fast(neighbor.0, Message::AreYouUsingTheSharedFork)
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

impl Reading for Philosopher {
    type Message = Message;
    type Codec = examples::PostcardCodec<Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        // a single-byte length prefix suffices for these tiny messages
        examples::PostcardCodec::new(1)
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) {
        let (left_neighbor_addr, left_neighbor_name) = self.left_neighbor.get().unwrap();
        let (neighbor_name, neighbor_side) = if source == *left_neighbor_addr {
            (left_neighbor_name.to_owned(), "left")
        } else {
            (self.right_neighbor.get().unwrap().1.to_owned(), "right")
        };

        match message {
            Message::AreYouUsingTheSharedFork => {
                let answer = if matches!(*self.state.read().await, State::Thinking) {
                    debug!(parent: self.node().span(), "giving {neighbor_name} my {neighbor_side} fork");
                    Message::No
                } else {
                    debug!(parent: self.node().span(), "I'm not giving {neighbor_name} my {neighbor_side} fork yet");
                    Message::Yes(None)
                };

                // deliberately assert on every layer: queueing, sending, and delivery
                self.unicast(source, answer)
                    .unwrap()
                    .await
                    .unwrap()
                    .unwrap();
            }
            Message::Yes(duration) => {
                debug!(parent: self.node().span(), "{neighbor_name} won't share his fork yet");

                let state = self.state.read().await;
                if *state != State::Hungry(true) {
                    drop(state);
                    *self.state.write().await = State::Thinking;
                } else if let Some(time) = duration {
                    sleep(time).await;
                }
            }
            Message::No => {
                info!(parent: self.node().span(), "I got the fork from {neighbor_name}");

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
    }
}

impl Writing for Philosopher {
    type Message = Message;
    type Codec = examples::PostcardCodec<Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        // a single-byte length prefix suffices for these tiny messages
        examples::PostcardCodec::new(1)
    }
}

#[tokio::main]
async fn main() {
    examples::start_logger(LevelFilter::INFO);

    let philosophers = vec![
        Philosopher::new("Socrates".to_owned()).await,
        Philosopher::new("Diogenes".to_owned()).await,
        Philosopher::new("Kant".to_owned()).await,
        Philosopher::new("Nietzsche".to_owned()).await,
        Philosopher::new("Wittgenstein".to_owned()).await,
    ];

    connect_nodes(&philosophers, Topology::Ring).await.unwrap();
    sleep(Duration::from_millis(100)).await;

    let count = philosophers.len();
    for (i, philosopher) in philosophers.iter().enumerate() {
        let right = &philosophers[(i + 1) % count];
        let left = &philosophers[(i + count - 1) % count];

        // the ring is directional: we connected *to* the right neighbor (at its
        // listening address), while the left one connected to us (so it is only
        // reachable via its ephemeral connection address)
        let right_neighbor_addr = right.node().listening_addr().await.unwrap();
        let both_neighbors = philosopher.node().connected_addrs();
        assert_eq!(both_neighbors.len(), 2);
        let left_neighbor_addr = both_neighbors
            .into_iter()
            .find(|addr| *addr != right_neighbor_addr)
            .unwrap();

        philosopher
            .right_neighbor
            .set((right_neighbor_addr, right.node().name().to_owned()))
            .unwrap();
        philosopher
            .left_neighbor
            .set((left_neighbor_addr, left.node().name().to_owned()))
            .unwrap();
    }

    for p in &philosophers {
        p.start_dining();
    }

    sleep(Duration::from_secs(60)).await;

    // nodes are never dropped implicitly; always shut them down once done
    for philosopher in &philosophers {
        philosopher.node().shut_down().await;
    }
}
