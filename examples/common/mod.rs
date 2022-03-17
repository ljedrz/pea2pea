#![allow(dead_code)]

use bytes::Buf;
use tracing_subscriber::filter::{EnvFilter, LevelFilter};

use std::io;

pub fn start_logger(default_level: LevelFilter) {
    let filter = match EnvFilter::try_from_default_env() {
        Ok(filter) => filter.add_directive("mio=off".parse().unwrap()),
        _ => EnvFilter::default()
            .add_directive(default_level.into())
            .add_directive("mio=off".parse().unwrap()),
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .without_time()
        .with_target(false)
        .init();
}

pub fn read_len_prefixed_message<R: Buf, const N: usize>(
    reader: &mut R,
) -> io::Result<Option<Vec<u8>>> {
    if reader.remaining() < N {
        return Ok(None);
    }

    let payload_len = match N {
        2 => reader.get_u16_le() as usize,
        4 => reader.get_u32_le() as usize,
        _ => unreachable!(),
    };

    if payload_len == 0 {
        return Err(io::ErrorKind::InvalidData.into());
    }

    if reader.remaining() < payload_len {
        return Ok(None);
    }

    let mut buffer = vec![0u8; payload_len];
    reader.take(payload_len).copy_to_slice(&mut buffer);

    Ok(Some(buffer))
}

pub fn prefix_with_len(len_size: usize, message: &[u8]) -> Vec<u8> {
    let mut vec = Vec::with_capacity(len_size + message.len());

    match len_size {
        2 => vec.extend_from_slice(&(message.len() as u16).to_le_bytes()),
        4 => vec.extend_from_slice(&(message.len() as u32).to_le_bytes()),
        _ => unreachable!(),
    }

    vec.extend_from_slice(message);

    vec
}
