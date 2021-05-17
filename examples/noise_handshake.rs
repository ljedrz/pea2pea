mod common;

use bytes::Bytes;
use parking_lot::{Mutex, RwLock};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::sleep,
};
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

use pea2pea::{
    protocols::{Handshaking, Reading, Writing},
    Connection, ConnectionSide, Node, NodeConfig, Pea2Pea,
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
    node: Node,
    noise_states: Arc<RwLock<HashMap<SocketAddr, Arc<Mutex<NoiseState>>>>>,
}

impl Pea2Pea for SecureNode {
    fn node(&self) -> &Node {
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
            Ok(Some(&buffer[2..][..payload_len]))
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
            listener_ip: "127.0.0.1".parse().unwrap(),
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

#[async_trait::async_trait]
impl Handshaking for SecureNode {
    async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
        // the noise handshake settings used by snow
        const HANDSHAKE_PATTERN: &str = "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s";
        const PRE_SHARED_KEY: &[u8] = b"I dont care for codes of conduct"; // the PSK must be 32B

        let builder = snow::Builder::new(HANDSHAKE_PATTERN.parse().unwrap());
        let static_key = builder.generate_keypair().unwrap().private;
        let noise_builder = builder
            .local_private_key(&static_key)
            .psk(3, PRE_SHARED_KEY);
        let mut buffer: Box<[u8]> = vec![0u8; NOISE_BUF_LEN].into();
        let mut buf = [0u8; NOISE_BUF_LEN]; // a temporary intermediate buffer to decrypt from

        let state = match !conn.side {
            ConnectionSide::Initiator => {
                let mut noise = noise_builder.build_initiator().unwrap();

                // -> e
                let len = noise.write_message(&[], &mut buffer).unwrap();
                conn.writer()
                    .write_all(&packet_message(&buffer[..len]))
                    .await?;
                debug!(parent: conn.node.span(), "sent e (XX handshake part 1/3)");

                // <- e, ee, s, es
                let len = conn.reader().read(&mut buf).await?;
                let message = read_message(&buf[..len])?.unwrap();
                noise.read_message(message, &mut buffer).unwrap();
                debug!(parent: conn.node.span(), "received e, ee, s, es (XX handshake part 2/3)");

                // -> s, se, psk
                let len = noise.write_message(&[], &mut buffer).unwrap();
                conn.writer()
                    .write_all(&packet_message(&buffer[..len]))
                    .await?;
                debug!(parent: conn.node.span(), "sent s, se, psk (XX handshake part 3/3)");

                noise.into_transport_mode().unwrap()
            }
            ConnectionSide::Responder => {
                let mut noise = noise_builder.build_responder().unwrap();

                // <- e
                let len = conn.reader().read(&mut buf).await?;
                let message = read_message(&buf[..len])?.unwrap();
                noise.read_message(message, &mut buffer).unwrap();
                debug!(parent: conn.node.span(), "received e (XX handshake part 1/3)");

                // -> e, ee, s, es
                let len = noise.write_message(&[], &mut buffer).unwrap();
                conn.writer()
                    .write_all(&packet_message(&buffer[..len]))
                    .await?;
                debug!(parent: conn.node.span(), "sent e, ee, s, es (XX handshake part 2/3)");

                // <- s, se, psk
                let len = conn.reader().read(&mut buf).await?;
                let message = read_message(&buf[..len])?.unwrap();
                noise.read_message(message, &mut buffer).unwrap();
                debug!(parent: conn.node.span(), "received s, se, psk (XX handshake part 3/3)");

                noise.into_transport_mode().unwrap()
            }
        };

        debug!(parent: conn.node.span(), "XX handshake complete");

        let noise_state = NoiseState { state, buffer };

        self.noise_states
            .write()
            .insert(conn.addr, Arc::new(Mutex::new(noise_state)));

        Ok(conn)
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

impl Writing for SecureNode {
    fn write_message(
        &self,
        target: SocketAddr,
        payload: &[u8],
        conn_buffer: &mut [u8],
    ) -> io::Result<usize> {
        let to_encrypt = str::from_utf8(payload).unwrap();
        info!(parent: self.node.span(), "sending an encrypted message to {}: \"{}\"", target, to_encrypt);

        let noise = Arc::clone(&self.noise_states.read().get(&target).unwrap());

        let NoiseState { state, buffer } = &mut *noise.lock();
        let len = state.write_message(payload, buffer).unwrap();
        let encrypted_message = &buffer[..len];

        conn_buffer[..2].copy_from_slice(&(len as u16).to_be_bytes());
        conn_buffer[2..][..len].copy_from_slice(&encrypted_message);

        Ok(2 + len)
    }
}

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::TRACE);

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
        .connect(responder.node().listening_addr())
        .await
        .unwrap();

    // allow a bit of time for the handshake process to conclude
    sleep(Duration::from_millis(10)).await;

    // send a message from initiator to responder
    let msg = b"why hello there, fellow noise protocol user; I'm the initiator";
    initiator
        .node()
        .send_direct_message(responder.node().listening_addr(), msg[..].into())
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
