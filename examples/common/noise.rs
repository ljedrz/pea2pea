use bytes::{Bytes, BytesMut};
use futures_util::{sink::SinkExt, TryStreamExt};
use tokio_util::codec::{Decoder, Encoder, Framed, FramedParts, LengthDelimitedCodec};
use tracing::*;

use pea2pea::{protocols::Handshake, Connection, ConnectionSide, Pea2Pea};

use std::{io, sync::Arc};

// maximum noise message size, as specified by its protocol
pub const NOISE_MAX_LEN: usize = 65535;

// an object representing the state of noise
pub enum NoiseState {
    Handshake(Box<snow::HandshakeState>),
    PostHandshake {
        // stateless state can be used immutably
        state: Arc<snow::StatelessTransportState>,
        // only used in Reading
        rx_nonce: Option<u64>,
        // only used in Writing
        tx_nonce: Option<u64>,
    },
}

// only post-handshake state is cloned (in the Reading impl)
impl Clone for NoiseState {
    fn clone(&self) -> Self {
        match self {
            Self::Handshake(..) => panic!("unsupported"),
            Self::PostHandshake {
                state,
                rx_nonce,
                tx_nonce,
            } => Self::PostHandshake {
                state: Arc::clone(state),
                rx_nonce: *rx_nonce,
                tx_nonce: *tx_nonce,
            },
        }
    }
}

impl NoiseState {
    // obtain the handshake noise state
    pub fn handshake(&mut self) -> Option<&mut snow::HandshakeState> {
        if let Self::Handshake(state) = self {
            Some(state)
        } else {
            None
        }
    }

    // transform the noise state into post-handshake
    pub fn into_post_handshake(self) -> Self {
        if let Self::Handshake(state) = self {
            let state = state.into_stateless_transport_mode().unwrap();
            Self::PostHandshake {
                state: Arc::new(state),
                rx_nonce: Some(0),
                tx_nonce: Some(0),
            }
        } else {
            panic!();
        }
    }

    // obtain the post-handshake noise state
    pub fn post_handshake(&self) -> &Arc<snow::StatelessTransportState> {
        if let Self::PostHandshake { state, .. } = self {
            state
        } else {
            panic!();
        }
    }

    // mutably borrow the receive nonce
    pub fn rx_nonce(&mut self) -> &mut u64 {
        if let Self::PostHandshake {
            rx_nonce: Some(ref mut val),
            ..
        } = self
        {
            val
        } else {
            panic!();
        }
    }

    // mutably borrow the send nonce
    pub fn tx_nonce(&mut self) -> &mut u64 {
        if let Self::PostHandshake {
            tx_nonce: Some(ref mut val),
            ..
        } = self
        {
            val
        } else {
            panic!();
        }
    }
}

// a codec used to (en/de)crypt messages using noise
pub struct NoiseCodec {
    codec: LengthDelimitedCodec,
    pub noise: NoiseState,
    buffer: Box<[u8]>,
}

impl NoiseCodec {
    pub fn new(noise: NoiseState) -> Self {
        NoiseCodec {
            codec: LengthDelimitedCodec::builder()
                .length_field_length(2)
                .new_codec(),
            noise,
            buffer: vec![0u8; NOISE_MAX_LEN].into(),
        }
    }
}

impl Decoder for NoiseCodec {
    type Item = Bytes;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // obtain the whole message first, using the length-delimited codec
        let bytes = if let Some(bytes) = self.codec.decode(src)? {
            bytes
        } else {
            return Ok(None);
        };

        // decrypt it in the handshake or post-handshake mode
        let msg_len = if let Some(noise) = self.noise.handshake() {
            noise
                .read_message(&bytes, &mut self.buffer)
                .map_err(|_| io::ErrorKind::InvalidData)?
        } else {
            let rx_nonce = *self.noise.rx_nonce();
            let len = self
                .noise
                .post_handshake()
                .read_message(rx_nonce, &bytes, &mut self.buffer)
                .map_err(|_| io::ErrorKind::InvalidData)?;
            *self.noise.rx_nonce() += 1;
            len
        };
        let msg = self.buffer[..msg_len].to_vec().into();

        Ok(Some(msg))
    }
}

impl Encoder<Bytes> for NoiseCodec {
    type Error = io::Error;

    fn encode(&mut self, msg: Bytes, dst: &mut BytesMut) -> Result<(), Self::Error> {
        // encrypt the message in the handshake or post-handshake mode
        let msg_len = if let Some(noise) = self.noise.handshake() {
            noise.write_message(&msg, &mut self.buffer).unwrap()
        } else {
            let tx_nonce = *self.noise.tx_nonce();
            let len = self
                .noise
                .post_handshake()
                .write_message(tx_nonce, &msg, &mut self.buffer)
                .map_err(|_| io::ErrorKind::InvalidData)?;
            *self.noise.tx_nonce() += 1;
            len
        };
        let msg: Bytes = self.buffer[..msg_len].to_vec().into();

        // encode it using the length-delimited codec
        self.codec.encode(msg, dst)
    }
}

// perform the noise XX handshake
pub async fn handshake_xx<'a, T: Pea2Pea + Handshake>(
    node: &T,
    conn: &mut Connection,
    noise_builder: snow::Builder<'a>,
    payload: Bytes,
) -> io::Result<(NoiseState, Bytes)> {
    let node_conn_side = !conn.side();
    let stream = node.borrow_stream(conn);

    let (noise_state, secure_payload) = match node_conn_side {
        ConnectionSide::Initiator => {
            let noise = Box::new(noise_builder.build_initiator().unwrap());
            let mut framed = Framed::new(stream, NoiseCodec::new(NoiseState::Handshake(noise)));

            // -> e
            framed.send("".into()).await?;
            debug!(parent: node.node().span(), "sent e (XX handshake part 1/3)");

            // <- e, ee, s, es
            let secure_payload = framed.try_next().await?.unwrap_or_default();
            debug!(parent: node.node().span(), "received e, ee, s, es (XX handshake part 2/3)");

            // -> s, se, psk
            framed.send(payload).await?;
            debug!(parent: node.node().span(), "sent s, se, psk (XX handshake part 3/3)");

            let FramedParts { codec, .. } = framed.into_parts();
            let NoiseCodec { noise, .. } = codec;

            (noise.into_post_handshake(), secure_payload)
        }
        ConnectionSide::Responder => {
            let noise = Box::new(noise_builder.build_responder().unwrap());
            let mut framed = Framed::new(stream, NoiseCodec::new(NoiseState::Handshake(noise)));

            // <- e
            framed.try_next().await?;
            debug!(parent: node.node().span(), "received e (XX handshake part 1/3)");

            // -> e, ee, s, es
            framed.send(payload).await?;
            debug!(parent: node.node().span(), "sent e, ee, s, es (XX handshake part 2/3)");

            // <- s, se, psk
            let secure_payload = framed.try_next().await?.unwrap_or_default();
            debug!(parent: node.node().span(), "received s, se, psk (XX handshake part 3/3)");

            let FramedParts { codec, .. } = framed.into_parts();
            let NoiseCodec { noise, .. } = codec;

            (noise.into_post_handshake(), secure_payload)
        }
    };

    debug!(parent: node.node().span(), "XX handshake complete");

    Ok((noise_state, secure_payload))
}
