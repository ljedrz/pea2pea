use bytes::Bytes;
use parking_lot::{Mutex, RwLock};
use tokio::{sync::mpsc, time::sleep};
use tracing::*;

use pea2pea::{
    connections::{ConnectionSide, ConnectionWriter},
    protocols::{Handshaking, Reading, Writing},
    Node, NodeConfig, Pea2Pea,
};

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

impl Pea2Pea for SecureNode {
    fn node(&self) -> &Arc<Node> {
        &self.node
    }
}

// read the first message from the provided buffer
fn read_message(buffer: &[u8]) -> io::Result<Option<&[u8]>> {
    // expecting the test messages to be prefixed with their length encoded as a BE u16
    if buffer.len() >= 2 {
        let payload_len = u16::from_be_bytes(buffer[..2].try_into().unwrap()) as usize;

        if payload_len == 0 {
            return Err(io::ErrorKind::InvalidData.into());
        }

        if buffer[2..].len() >= payload_len {
            Ok(Some(&buffer[2..2 + payload_len]))
        } else {
            Ok(None)
        }
    } else {
        Ok(None)
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
        let config = NodeConfig {
            name: Some(name.into()),
            conn_read_buffer_size: NOISE_BUF_LEN + 2, // 2 for the encrypted message length,
            ..Default::default()
        };
        let node = Node::new(Some(config)).await?;

        Ok(Self {
            node,
            noise_states: Default::default(),
        })
    }
}

impl Handshaking for SecureNode {
    fn enable_handshaking(&self) {
        let (from_node_sender, mut from_node_receiver) = mpsc::channel(1);
        self.node().set_handshake_handler(from_node_sender.into());

        // the noise handshake settings used by snow
        const HANDSHAKE_PATTERN: &str = "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s";
        const PRE_SHARED_KEY: &[u8] = b"I dont care for codes of conduct"; // the PSK must be 32B

        // spawn a background task dedicated to handling the handshakes
        let self_clone = self.clone();
        tokio::spawn(async move {
            loop {
                if let Some((mut conn_reader, mut conn_writer, node_side, result_sender)) =
                    from_node_receiver.recv().await
                {
                    let addr = conn_reader.addr;

                    let noise_state = match node_side {
                        ConnectionSide::Initiator => {
                            info!(parent: conn_reader.node.span(), "handshaking with {} as the initiator", addr);

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
                            conn_writer
                                .write_all(&packet_message(&buffer[..len]))
                                .await
                                .unwrap();

                            // <- e, ee, s, es
                            let queued_bytes = conn_reader.read_queued_bytes().await.unwrap();
                            let message = read_message(queued_bytes).unwrap().unwrap();
                            noise.read_message(message, &mut buffer).unwrap();

                            // -> s, se, psk
                            let len = noise.write_message(&[], &mut buffer).unwrap();
                            conn_writer
                                .write_all(&packet_message(&buffer[..len]))
                                .await
                                .unwrap();

                            NoiseState {
                                state: noise.into_transport_mode().unwrap(),
                                buffer,
                            }
                        }
                        ConnectionSide::Responder => {
                            info!(parent: conn_reader.node.span(), "handshaking with {} as the responder", addr);

                            let builder = snow::Builder::new(HANDSHAKE_PATTERN.parse().unwrap());
                            let static_key = builder.generate_keypair().unwrap().private;
                            let mut noise = builder
                                .local_private_key(&static_key)
                                .psk(3, PRE_SHARED_KEY)
                                .build_responder()
                                .unwrap();
                            let mut buffer: Box<[u8]> = vec![0u8; NOISE_BUF_LEN].into();

                            // <- e
                            let queued_bytes = conn_reader.read_queued_bytes().await.unwrap();
                            let message = read_message(queued_bytes).unwrap().unwrap();
                            noise.read_message(message, &mut buffer).unwrap();

                            // -> e, ee, s, es
                            let len = noise.write_message(&[], &mut buffer).unwrap();
                            conn_writer
                                .write_all(&packet_message(&buffer[..len]))
                                .await
                                .unwrap();

                            // <- s, se, psk
                            let queued_bytes = conn_reader.read_queued_bytes().await.unwrap();
                            let message = read_message(queued_bytes).unwrap().unwrap();
                            noise.read_message(message, &mut buffer).unwrap();

                            NoiseState {
                                state: noise.into_transport_mode().unwrap(),
                                buffer,
                            }
                        }
                    };

                    let noise_state = Arc::new(Mutex::new(noise_state));
                    self_clone.noise_states.write().insert(addr, noise_state);

                    // return the connection objects to the node
                    if result_sender.send(Ok((conn_reader, conn_writer))).is_err() {
                        // can't recover if this happens
                        unreachable!();
                    }
                }
            }
        });
    }
}

#[async_trait::async_trait]
impl Reading for SecureNode {
    type Message = String;

    fn read_message(
        &self,
        source: SocketAddr,
        buffer: &[u8],
    ) -> io::Result<Option<(Self::Message, usize)>> {
        let bytes = read_message(buffer)?;

        if let Some(bytes) = bytes {
            let noise = Arc::clone(self.noise_states.read().get(&source).unwrap());
            let NoiseState { state, buffer } = &mut *noise.lock();

            let len = state.read_message(bytes, buffer).ok().unwrap();
            let decrypted_message = String::from_utf8(buffer[..len].to_vec()).unwrap();

            // account for the length prefix discarded in read_message
            Ok(Some((decrypted_message, bytes.len() + 2)))
        } else {
            Ok(None)
        }
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()> {
        info!(parent: self.node().span(), "decrypted a message from {}: \"{}\"", source, message);

        Ok(())
    }
}

#[async_trait::async_trait]
impl Writing for SecureNode {
    async fn write_message(&self, writer: &mut ConnectionWriter, payload: &[u8]) -> io::Result<()> {
        let to_encrypt = str::from_utf8(payload).unwrap();
        let target = writer.addr;
        info!(parent: self.node.span(), "sending an encrypted message to {}: \"{}\"", target, to_encrypt);

        let noise = Arc::clone(&self.noise_states.read().get(&target).unwrap());

        let message = {
            let NoiseState { state, buffer } = &mut *noise.lock();
            let len = state.write_message(payload, buffer).unwrap();
            let encrypted_message = &buffer[..len];
            packet_message(encrypted_message)
        };

        writer.write_all(&message).await
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let initiator = SecureNode::new("initiator").await.unwrap();
    let responder = SecureNode::new("responder").await.unwrap();

    for node in &[&initiator, &responder] {
        node.enable_handshaking(); // enable the pre-defined handshakes
        node.enable_reading(); // enable the reading protocol
        node.enable_writing(); // enable the writing protocol
    }

    // connect the initiator to the responder
    initiator
        .node()
        .initiate_connection(responder.node().listening_addr)
        .await
        .unwrap();

    // allow a bit of time for the handshake process to conclude
    sleep(Duration::from_millis(10)).await;

    // send a message from initiator to responder
    let msg = b"why hello there, fellow noise protocol user; I'm the initiator";
    initiator
        .node()
        .send_direct_message(responder.node().listening_addr, msg[..].into())
        .await
        .unwrap();

    // send a message from responder to initiator; determine the latter's address first
    let initiator_addr = responder.node().connected_addrs()[0];
    let msg = b"why hello there, fellow noise protocol user; I'm the responder";
    responder
        .node()
        .send_direct_message(initiator_addr, msg[..].into())
        .await
        .unwrap();

    sleep(Duration::from_millis(10)).await;
}
