mod common;
use common::{prefix_with_len, read_len_prefixed_message};

use parking_lot::{Mutex, RwLock};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::sleep,
};
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

use pea2pea::{
    protocols::{Handshake, Reading, Writing},
    Config, Connection, ConnectionSide, Node, Pea2Pea,
};

use std::{collections::HashMap, io, net::SocketAddr, str, sync::Arc, time::Duration};

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

impl SecureNode {
    // create a SecureNode
    async fn new(name: &str) -> io::Result<Self> {
        let config = Config {
            name: Some(name.into()),
            read_buffer_size: NOISE_BUF_LEN + 2, // 2 for the encrypted message length,
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
impl Handshake for SecureNode {
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
                    .write_all(&prefix_with_len(2, &buffer[..len]))
                    .await?;
                debug!(parent: self.node().span(), "sent e (XX handshake part 1/3)");

                // <- e, ee, s, es
                conn.reader().read(&mut buf).await?;
                let message =
                    read_len_prefixed_message::<_, 2>(&mut io::Cursor::new(buf))?.unwrap();
                noise.read_message(&message, &mut buffer).unwrap();
                debug!(parent: self.node().span(), "received e, ee, s, es (XX handshake part 2/3)");

                // -> s, se, psk
                let len = noise.write_message(&[], &mut buffer).unwrap();
                conn.writer()
                    .write_all(&prefix_with_len(2, &buffer[..len]))
                    .await?;
                debug!(parent: self.node().span(), "sent s, se, psk (XX handshake part 3/3)");

                noise.into_transport_mode().unwrap()
            }
            ConnectionSide::Responder => {
                let mut noise = noise_builder.build_responder().unwrap();

                // <- e
                conn.reader().read(&mut buf).await?;
                let message =
                    read_len_prefixed_message::<_, 2>(&mut io::Cursor::new(buf))?.unwrap();
                noise.read_message(&message, &mut buffer).unwrap();
                debug!(parent: self.node().span(), "received e (XX handshake part 1/3)");

                // -> e, ee, s, es
                let len = noise.write_message(&[], &mut buffer).unwrap();
                conn.writer()
                    .write_all(&prefix_with_len(2, &buffer[..len]))
                    .await?;
                debug!(parent: self.node().span(), "sent e, ee, s, es (XX handshake part 2/3)");

                // <- s, se, psk
                conn.reader().read(&mut buf).await?;
                let message =
                    read_len_prefixed_message::<_, 2>(&mut io::Cursor::new(buf))?.unwrap();
                noise.read_message(&message, &mut buffer).unwrap();
                debug!(parent: self.node().span(), "received s, se, psk (XX handshake part 3/3)");

                noise.into_transport_mode().unwrap()
            }
        };

        debug!(parent: self.node().span(), "XX handshake complete");

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

    fn read_message<R: io::Read>(
        &self,
        source: SocketAddr,
        reader: &mut R,
    ) -> io::Result<Option<Self::Message>> {
        let bytes = read_len_prefixed_message::<_, 2>(reader)?;

        if let Some(bytes) = bytes {
            let noise = Arc::clone(self.noise_states.read().get(&source).unwrap());
            let NoiseState { state, buffer } = &mut *noise.lock();

            let len = state.read_message(&bytes, buffer).ok().unwrap();
            let decrypted_message = String::from_utf8(buffer[..len].to_vec()).unwrap();

            Ok(Some(decrypted_message))
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
    type Message = String;

    fn write_message<W: io::Write>(
        &self,
        target: SocketAddr,
        payload: &Self::Message,
        writer: &mut W,
    ) -> io::Result<()> {
        info!(parent: self.node.span(), "sending an encrypted message to {}: \"{}\"", target, payload);

        let noise = Arc::clone(self.noise_states.read().get(&target).unwrap());

        let NoiseState { state, buffer } = &mut *noise.lock();
        let len = state.write_message(payload.as_bytes(), buffer).unwrap();
        let encrypted_message = &buffer[..len];

        writer.write_all(&(len as u16).to_le_bytes())?;
        writer.write_all(encrypted_message)
    }
}

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::TRACE);

    let initiator = SecureNode::new("initiator").await.unwrap();
    let responder = SecureNode::new("responder").await.unwrap();

    for node in &[&initiator, &responder] {
        node.enable_handshake(); // enable the pre-defined handshakes
        node.enable_reading(); // enable the reading protocol
        node.enable_writing(); // enable the writing protocol
    }

    // connect the initiator to the responder
    initiator
        .node()
        .connect(responder.node().listening_addr().unwrap())
        .await
        .unwrap();

    // allow a bit of time for the handshake process to conclude
    sleep(Duration::from_millis(10)).await;

    // send a message from initiator to responder
    let msg = "why hello there, fellow noise protocol user; I'm the initiator";
    initiator
        .send_direct_message(responder.node().listening_addr().unwrap(), msg.to_string())
        .unwrap();

    // send a message from responder to initiator; determine the latter's address first
    let initiator_addr = responder.node().connected_addrs()[0];
    let msg = "why hello there, fellow noise protocol user; I'm the responder";
    responder
        .send_direct_message(initiator_addr, msg.to_string())
        .unwrap();

    sleep(Duration::from_millis(10)).await;
}
