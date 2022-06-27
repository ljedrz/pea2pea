//! A `snow`-powered implementation of the noise XX handshake for `pea2pea`.

use bytes::{Bytes, BytesMut};
use futures_util::{sink::SinkExt, TryStreamExt};
use tokio_util::codec::{Decoder, Encoder, Framed, FramedParts, LengthDelimitedCodec};
use tracing::*;

use pea2pea::{protocols::Handshake, Connection, ConnectionSide};

use std::{io, sync::Arc};

// maximum noise message size, as specified by its protocol
pub const MAX_MESSAGE_LEN: usize = 65535;

// an object representing the state of noise
pub enum State {
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
impl Clone for State {
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

impl State {
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
    pub fn post_handshake(&self) -> &snow::StatelessTransportState {
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
pub struct Codec {
    codec: LengthDelimitedCodec,
    pub noise: State,
    buffer: Box<[u8]>,
}

impl Codec {
    pub fn new(noise: State) -> Self {
        Codec {
            codec: LengthDelimitedCodec::builder()
                .length_field_length(2)
                .new_codec(),
            noise,
            buffer: vec![0u8; MAX_MESSAGE_LEN].into(),
        }
    }
}

impl Decoder for Codec {
    type Item = BytesMut;
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
        let msg = self.buffer[..msg_len].into();

        Ok(Some(msg))
    }
}

impl Encoder<Bytes> for Codec {
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
pub async fn handshake_xx<'a, T: Handshake>(
    node: &T,
    conn: &mut Connection,
    noise_builder: snow::Builder<'a>,
    payload: Bytes,
) -> io::Result<(State, Bytes)> {
    let node_conn_side = !conn.side();
    let stream = node.borrow_stream(conn);

    let (noise_state, secure_payload) = match node_conn_side {
        ConnectionSide::Initiator => {
            let noise = Box::new(noise_builder.build_initiator().unwrap());
            let mut framed = Framed::new(stream, Codec::new(State::Handshake(noise)));

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
            let Codec { noise, .. } = codec;

            (noise.into_post_handshake(), secure_payload)
        }
        ConnectionSide::Responder => {
            let noise = Box::new(noise_builder.build_responder().unwrap());
            let mut framed = Framed::new(stream, Codec::new(State::Handshake(noise)));

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
            let Codec { noise, .. } = codec;

            (noise.into_post_handshake(), secure_payload)
        }
    };

    debug!(parent: node.node().span(), "XX handshake complete");

    Ok((noise_state, secure_payload.freeze()))
}
