use parking_lot::{Mutex, RwLock};
use tokio::{io::AsyncReadExt, sync::mpsc::channel, task::JoinHandle, time::sleep};
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
        let u16_len_header = (len as u16).to_be_bytes();

        self.node()
            .send_direct_message(target, Some(&u16_len_header), encrypted_message)
            .await
    }
}

// read a packeted message
async fn receive_message(connection_reader: &mut ConnectionReader) -> std::io::Result<&[u8]> {
    // expecting the messages to be prefixed with their length encoded as a BE u16
    let msg_len_size: usize = 2;

    let buffer = &mut connection_reader.buffer;
    connection_reader
        .reader
        .read_exact(&mut buffer[..msg_len_size])
        .await?;
    let msg_len = u16::from_be_bytes(buffer[..msg_len_size].try_into().unwrap()) as usize;
    connection_reader
        .reader
        .read_exact(&mut buffer[..msg_len])
        .await?;

    Ok(&buffer[..msg_len])
}

impl HandshakeProtocol for SecureNode {
    fn enable_handshake_protocol(&self) {
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
                let len = noise.write_message(&[], &mut buffer[2..]).unwrap();
                buffer[..2].copy_from_slice(&(len as u16).to_be_bytes());
                connection.write_bytes(&buffer[..len + 2]).await.unwrap();

                // <- e, ee, s, es
                let message = receive_message(&mut connection_reader).await.unwrap();
                noise.read_message(message, &mut buffer).unwrap();

                // -> s, se, psk
                let len = noise.write_message(&[], &mut buffer[2..]).unwrap();
                buffer[..2].copy_from_slice(&(len as u16).to_be_bytes());
                connection.write_bytes(&buffer[..len + 2]).await.unwrap();

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
                let message = receive_message(&mut connection_reader).await.unwrap();
                noise.read_message(message, &mut buffer).unwrap();

                // -> e, ee, s, es
                let len = noise.write_message(&[], &mut buffer[2..]).unwrap();
                buffer[..2].copy_from_slice(&(len as u16).to_be_bytes());
                connection.write_bytes(&buffer[..len + 2]).await.unwrap();

                // <- s, se, psk
                let message = receive_message(&mut connection_reader).await.unwrap();
                noise.read_message(message, &mut buffer).unwrap();

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
impl MessagingProtocol for SecureNode {
    type Message = String; // the encrypted messages are strings

    async fn receive_message(connection_reader: &mut ConnectionReader) -> std::io::Result<&[u8]> {
        receive_message(connection_reader).await
    }

    fn parse_message(&self, source: SocketAddr, message: &[u8]) -> Option<Self::Message> {
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
        node.enable_handshake_protocol(); // enable the pre-defined handshakes
        node.enable_messaging_protocol(); // enable the pre-defined messaging rules
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
