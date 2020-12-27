use bytes::Bytes;
use parking_lot::{Mutex, RwLock};
use tokio::{sync::mpsc::channel, task::JoinHandle, time::sleep};
use tracing::*;

use pea2pea::*;

use std::{
    collections::HashMap, convert::TryInto, io, net::SocketAddr, str, sync::Arc, time::Duration,
};

// maximum noise message size, as specified by its protocol
const NOISE_BUF_LEN: usize = 65535;

struct NoiseState {
    state: snow::TransportState,
    buffer: Box<[u8]>, // an encryption/decryption buffer
}

#[derive(Clone)]
struct SecureNode {
    node: Arc<Node>,
    noise_states: Arc<RwLock<HashMap<SocketAddr, Arc<Mutex<NoiseState>>>>>,
}

impl ContainsNode for SecureNode {
    fn node(&self) -> &Arc<Node> {
        &self.node
    }
}

// read the first message from the provided buffer
fn read_message(buffer: &[u8]) -> Option<&[u8]> {
    // expecting the test messages to be prefixed with their length encoded as a BE u16
    if buffer.len() >= 2 {
        let payload_len = u16::from_be_bytes(buffer[..2].try_into().unwrap()) as usize;

        if buffer[2..].len() >= payload_len {
            Some(&buffer[..2 + payload_len])
        } else {
            None
        }
    } else {
        None
    }
}

// prepend the given message with its length encoded as a BE u16
fn packet_message(message: &[u8]) -> Bytes {
    let mut bytes = Vec::with_capacity(2 + message.len());
    let u16_len_header = (message.len() as u16).to_be_bytes();
    bytes.extend_from_slice(&u16_len_header);
    bytes.extend_from_slice(message);

    bytes.into()
}

impl SecureNode {
    // create a SecureNode
    async fn new(name: &str) -> io::Result<Self> {
        let mut config = NodeConfig::default();
        config.name = Some(name.into());
        config.conn_read_buffer_size = NOISE_BUF_LEN + 2; // 2 for the encrypted message length
        let node = Node::new(Some(config)).await?;

        Ok(Self {
            node,
            noise_states: Default::default(),
        })
    }

    // send a direct message encrypted using noise
    async fn send_direct_message(&self, target: SocketAddr, message: &[u8]) -> io::Result<()> {
        info!(parent: self.node.span(), "sending an encrypted message to {}: \"{}\"", target, str::from_utf8(message).unwrap());

        let noise = Arc::clone(&self.noise_states.read().get(&target).unwrap());
        let NoiseState { state, buffer } = &mut *noise.lock();

        let len = state.write_message(message, buffer).unwrap();
        let encrypted_message = &buffer[..len];

        self.node()
            .send_direct_message(target, packet_message(encrypted_message))
            .await
    }
}

impl Handshaking for SecureNode {
    fn enable_handshaking(&self) {
        // a channel used to register handshake states
        let (state_sender, mut state_receiver) = channel::<(SocketAddr, HandshakeState)>(64);

        let self_clone = self.clone();
        // a task registering the handshake states returned by the closures below
        tokio::spawn(async move {
            loop {
                if let Some((addr, state)) = state_receiver.recv().await {
                    let state: NoiseState = *state.downcast().unwrap();
                    let state = Arc::new(Mutex::new(state));
                    self_clone.noise_states.write().insert(addr, state);
                }
            }
        });

        // the noise handshake settings used by snow
        const HANDSHAKE_PATTERN: &str = "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s";
        const PRE_SHARED_KEY: &[u8] = b"I dont care for codes of conduct"; // the PSK must be 32B

        // the initiator's handshake closure
        let initiator = |addr: SocketAddr,
                         mut connection_reader: ConnectionReader,
                         connection: Arc<Connection>|
         -> JoinHandle<io::Result<(ConnectionReader, HandshakeState)>> {
            tokio::spawn(async move {
                info!(parent: connection_reader.node.span(), "handshaking with {} as the initiator", addr);

                let builder = snow::Builder::new(HANDSHAKE_PATTERN.parse().unwrap());
                let static_key = builder.generate_keypair().unwrap().private;
                let mut noise = builder
                    .local_private_key(&static_key)
                    .psk(3, PRE_SHARED_KEY)
                    .build_initiator()
                    .unwrap();
                let mut buffer: Box<[u8]> = vec![0u8; NOISE_BUF_LEN].into();

                // -> e
                let len = noise.write_message(&[], &mut buffer).unwrap();
                connection
                    .send_message(packet_message(&buffer[..len]))
                    .await;

                // <- e, ee, s, es
                let queued_bytes = connection_reader.read_queued_bytes().await.unwrap();
                let message = read_message(queued_bytes).unwrap();
                noise.read_message(&message[2..], &mut buffer).unwrap();

                // -> s, se, psk
                let len = noise.write_message(&[], &mut buffer).unwrap();
                connection
                    .send_message(packet_message(&buffer[..len]))
                    .await;

                let noise = NoiseState {
                    state: noise.into_transport_mode().unwrap(),
                    buffer,
                };

                Ok((connection_reader, Box::new(noise) as HandshakeState))
            })
        };

        // the responder's handshake closure
        let responder = |addr: SocketAddr,
                         mut connection_reader: ConnectionReader,
                         connection: Arc<Connection>|
         -> JoinHandle<io::Result<(ConnectionReader, HandshakeState)>> {
            tokio::spawn(async move {
                info!(parent: connection_reader.node.span(), "handshaking with {} as the responder", addr);

                let builder = snow::Builder::new(HANDSHAKE_PATTERN.parse().unwrap());
                let static_key = builder.generate_keypair().unwrap().private;
                let mut noise = builder
                    .local_private_key(&static_key)
                    .psk(3, PRE_SHARED_KEY)
                    .build_responder()
                    .unwrap();
                let mut buffer: Box<[u8]> = vec![0u8; NOISE_BUF_LEN].into();

                // <- e
                let queued_bytes = connection_reader.read_queued_bytes().await.unwrap();
                let message = read_message(queued_bytes).unwrap();
                noise.read_message(&message[2..], &mut buffer).unwrap();

                // -> e, ee, s, es
                let len = noise.write_message(&[], &mut buffer).unwrap();
                connection
                    .send_message(packet_message(&buffer[..len]))
                    .await;

                // <- s, se, psk
                let queued_bytes = connection_reader.read_queued_bytes().await.unwrap();
                let message = read_message(queued_bytes).unwrap();
                noise.read_message(&message[2..], &mut buffer).unwrap();

                let noise = NoiseState {
                    state: noise.into_transport_mode().unwrap(),
                    buffer,
                };

                Ok((connection_reader, Box::new(noise) as HandshakeState))
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

#[async_trait::async_trait]
impl Messaging for SecureNode {
    type Message = String; // the encrypted messages are strings

    fn read_message(buffer: &[u8]) -> Option<&[u8]> {
        read_message(buffer)
    }

    fn parse_message(&self, source: SocketAddr, message: &[u8]) -> Option<Self::Message> {
        // disregard the length prefix
        let message = &message[2..];

        let noise = Arc::clone(self.noise_states.read().get(&source)?);
        let NoiseState { state, buffer } = &mut *noise.lock();

        let len = state.read_message(&message, buffer).ok()?;
        let decrypted_message = String::from_utf8(buffer[..len].to_vec()).ok()?;

        Some(decrypted_message)
    }

    fn process_message(&self, source: SocketAddr, message: &Self::Message) {
        info!(parent: self.node().span(), "decrypted a message from {}: \"{}\"", source, message);
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let initiator = SecureNode::new("initiator").await.unwrap();
    let responder = SecureNode::new("responder").await.unwrap();

    for node in &[&initiator, &responder] {
        node.enable_handshaking(); // enable the pre-defined handshakes
        node.enable_messaging(); // enable the pre-defined messaging rules
    }

    // connect the initiator to the responder
    initiator
        .node
        .initiate_connection(responder.node().listening_addr)
        .await
        .unwrap();

    // allow a bit of time for the handshake process to conclude
    sleep(Duration::from_millis(100)).await;

    // send a message from initiator to responder
    let msg = b"why hello there, fellow noise protocol user; I'm the initiator";
    initiator
        .send_direct_message(responder.node().listening_addr, msg)
        .await
        .unwrap();

    // send a message from responder to initiator; determine the latter's address first
    let initiator_addr = responder.node().handshaken_addrs()[0];
    let msg = b"why hello there, fellow noise protocol user; I'm the responder";
    responder
        .send_direct_message(initiator_addr, msg)
        .await
        .unwrap();

    sleep(Duration::from_millis(100)).await;
}
