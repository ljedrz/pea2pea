#![allow(dead_code)]

use tracing_subscriber::filter::{EnvFilter, LevelFilter};

use std::{
    convert::TryInto,
    io::{self, Read},
};

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

pub fn read_len_prefixed_message<R: io::Read, const N: usize>(
    reader: &mut R,
) -> io::Result<Option<Vec<u8>>> {
    let mut len_arr = [0u8; N];
    if reader.read_exact(&mut len_arr).is_err() {
        return Ok(None);
    }
    let payload_len = match N {
        2 => u16::from_le_bytes(len_arr[..].try_into().unwrap()) as usize,
        4 => u32::from_le_bytes(len_arr[..].try_into().unwrap()) as usize,
        _ => unreachable!(),
    };

    if payload_len == 0 {
        return Err(io::ErrorKind::InvalidData.into());
    }

    let mut buffer = vec![0u8; payload_len];
    if reader
        .take(payload_len as u64)
        .read_exact(&mut buffer)
        .is_err()
    {
        Ok(None)
    } else {
        Ok(Some(buffer))
    }
}
