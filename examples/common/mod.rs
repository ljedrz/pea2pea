#![allow(dead_code)]

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

pub struct TestCodec<M>(pub LengthDelimitedCodec, PhantomData<M>);

impl Decoder for TestCodec<BytesMut> {
    type Item = BytesMut;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.0.decode(src)
    }
}

impl Decoder for TestCodec<String> {
    type Item = String;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.0
            .decode(src)?
            .map(|bs| String::from_utf8(bs.to_vec()).map_err(|_| io::ErrorKind::InvalidData.into()))
            .transpose()
    }
}

impl<M, T: Into<Bytes>> Encoder<T> for TestCodec<M> {
    type Error = io::Error;

    fn encode(&mut self, item: T, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.0.encode(item.into(), dst)
    }
}

impl<M> Default for TestCodec<M> {
    fn default() -> Self {
        let inner = LengthDelimitedCodec::builder()
            .length_field_length(2)
            .new_codec();
        Self(inner, PhantomData)
    }
}
