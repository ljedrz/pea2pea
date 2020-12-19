use crate::config::{ByteOrder::*, NodeConfig};

use tokio::{
    io::AsyncReadExt,
    net::tcp::{OwnedReadHalf, OwnedWriteHalf},
    task::JoinHandle,
};

use std::{convert::TryInto, io, sync::Arc};

pub(crate) struct ConnectionReader {
    config: Arc<NodeConfig>,
    buffer: Vec<u8>,
    reader: OwnedReadHalf,
}

macro_rules! read_msg_len {
    ($byte_order: expr, $msg_len: expr, $encoded_len: expr) => {{
        let len_bytes = $encoded_len;
        match ($byte_order, $msg_len) {
            (_, 1) => len_bytes[0] as usize,
            (BE, 2) => u16::from_be_bytes(len_bytes.try_into().unwrap()) as usize,
            (LE, 2) => u16::from_be_bytes(len_bytes.try_into().unwrap()) as usize,
            (BE, 4) => u32::from_be_bytes(len_bytes.try_into().unwrap()) as usize,
            (LE, 4) => u32::from_be_bytes(len_bytes.try_into().unwrap()) as usize,
            (BE, 8) => u64::from_be_bytes(len_bytes.try_into().unwrap()) as usize,
            (LE, 8) => u64::from_be_bytes(len_bytes.try_into().unwrap()) as usize,
            _ => unimplemented!(),
        }
    }};
}

impl ConnectionReader {
    pub(crate) fn new(reader: OwnedReadHalf, config: Arc<NodeConfig>) -> Self {
        Self {
            buffer: vec![0; config.conn_read_buffer_size],
            config,
            reader,
        }
    }

    pub(crate) async fn read_message(&mut self) -> io::Result<usize> {
        let msg_len_size = self.config.message_length_size as usize;
        self.reader
            .read_exact(&mut self.buffer[..msg_len_size])
            .await?;
        let msg_len = read_msg_len!(
            self.config.byte_order,
            msg_len_size,
            &self.buffer[..msg_len_size]
        );
        if msg_len > self.buffer.len() {
            // TODO: retun a nice io::Error instead
            panic!(
                "incoming message exceeded connection read buffer size: {} > {}",
                msg_len,
                self.buffer.len()
            );
        }
        self.reader.read_exact(&mut self.buffer[..msg_len]).await?;

        Ok(msg_len)
    }
}

pub(crate) struct Connection {
    reader_task: JoinHandle<()>,
    writer: OwnedWriteHalf,
}

impl Connection {
    pub(crate) fn new(reader_task: JoinHandle<()>, writer: OwnedWriteHalf) -> Self {
        Self {
            reader_task,
            writer,
        }
    }
}
