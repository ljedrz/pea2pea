pub mod noise;
pub mod yamux;

use std::{io, marker::PhantomData};

use bytes::{Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec};
use tracing_subscriber::filter::{EnvFilter, LevelFilter};

pub fn start_logger(default_level: LevelFilter) {
    let filter = match EnvFilter::try_from_default_env() {
        Ok(filter) => filter.add_directive("tokio_util=off".parse().unwrap()),
        _ => EnvFilter::default()
            .add_directive(default_level.into())
            .add_directive("tokio_util=off".parse().unwrap()),
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .without_time()
        .with_target(false)
        .init();
}

/// A minimal length-delimited codec for plain `BytesMut`/`String` messages.
pub struct SimpleCodec<M>(pub LengthDelimitedCodec, PhantomData<M>);

impl Decoder for SimpleCodec<BytesMut> {
    type Item = BytesMut;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.0.decode(src)
    }
}

impl Decoder for SimpleCodec<String> {
    type Item = String;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.0
            .decode(src)?
            .map(|bs| String::from_utf8(bs.to_vec()).map_err(|_| io::ErrorKind::InvalidData.into()))
            .transpose()
    }
}

impl<M, T: Into<Bytes>> Encoder<T> for SimpleCodec<M> {
    type Error = io::Error;

    fn encode(&mut self, item: T, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.0.encode(item.into(), dst)
    }
}

impl<M> Default for SimpleCodec<M> {
    fn default() -> Self {
        let inner = LengthDelimitedCodec::builder()
            .length_field_length(2)
            .new_codec();
        Self(inner, PhantomData)
    }
}

/// A codec for any postcard-serializable message type, sent over a length-delimited
/// transport; `length_field_len` is the width of the length prefix in bytes.
pub struct PostcardCodec<M>(pub LengthDelimitedCodec, PhantomData<M>);

impl<M> PostcardCodec<M> {
    pub fn new(length_field_len: usize) -> Self {
        let inner = LengthDelimitedCodec::builder()
            .length_field_length(length_field_len)
            .new_codec();
        Self(inner, PhantomData)
    }
}

impl<M> Default for PostcardCodec<M> {
    fn default() -> Self {
        Self::new(2)
    }
}

impl<M: serde::de::DeserializeOwned> Decoder for PostcardCodec<M> {
    type Item = M;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.0
            .decode(src)?
            .map(|bytes| {
                postcard::from_bytes(&bytes).map_err(|_| io::ErrorKind::InvalidData.into())
            })
            .transpose()
    }
}

impl<M: serde::Serialize> Encoder<M> for PostcardCodec<M> {
    type Error = io::Error;

    fn encode(&mut self, item: M, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let bytes =
            postcard::to_stdvec(&item).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
        self.0.encode(bytes.into(), dst)
    }
}

/// Waits until the node has at least one connection, and returns the address of the
/// first one; unlike a fixed sleep, this cannot race the connection setup.
pub async fn await_connection(node: &pea2pea::Node) -> std::net::SocketAddr {
    loop {
        if let Some(addr) = node.connected_addrs().first().copied() {
            return addr;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
}

pub fn check_for_24(err: &io::Error) {
    if let Some(24) = err.raw_os_error() {
        eprintln!("\nToo many open files! You need to increase your ulimit or reduce NUM_NODES.");
    }
}
