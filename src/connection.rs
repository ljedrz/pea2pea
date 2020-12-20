use crate::config::ByteOrder::*;
use crate::{Node, NodeConfig};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::tcp::{OwnedReadHalf, OwnedWriteHalf},
    sync::Mutex,
    task::JoinHandle,
};

use std::{convert::TryInto, io, sync::Arc};

pub(crate) struct ConnectionReader {
    node: Arc<Node>,
    buffer: Vec<u8>,
    reader: OwnedReadHalf,
}

macro_rules! read_msg_len {
    ($byte_order: expr, $msg_len: expr, $encoded_len: expr) => {{
        let len_bytes = $encoded_len;
        match ($byte_order, $msg_len) {
            (_, 1) => len_bytes[0] as usize,
            (BE, 2) => u16::from_be_bytes(len_bytes.try_into().unwrap()) as usize,
            (LE, 2) => u16::from_le_bytes(len_bytes.try_into().unwrap()) as usize,
            (BE, 4) => u32::from_be_bytes(len_bytes.try_into().unwrap()) as usize,
            (LE, 4) => u32::from_le_bytes(len_bytes.try_into().unwrap()) as usize,
            (BE, 8) => u64::from_be_bytes(len_bytes.try_into().unwrap()) as usize,
            (LE, 8) => u64::from_le_bytes(len_bytes.try_into().unwrap()) as usize,
            _ => unimplemented!(),
        }
    }};
}

impl ConnectionReader {
    pub(crate) fn new(reader: OwnedReadHalf, node: Arc<Node>) -> Self {
        Self {
            buffer: vec![0; node.config.conn_read_buffer_size],
            node,
            reader,
        }
    }

    fn config(&self) -> &NodeConfig {
        &self.node.config
    }

    pub(crate) async fn read_message(&mut self) -> io::Result<Vec<u8>> {
        let msg_len_size = self.config().message_length_size as usize;
        self.reader
            .read_exact(&mut self.buffer[..msg_len_size])
            .await?;
        let msg_len = read_msg_len!(
            self.config().message_byte_order,
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

        Ok(self.buffer[..msg_len].to_vec())
    }
}

pub(crate) struct Connection {
    node: Arc<Node>,
    _reader_task: JoinHandle<()>,
    writer: OwnedWriteHalf,
}

impl Connection {
    pub(crate) fn new(
        _reader_task: JoinHandle<()>,
        writer: OwnedWriteHalf,
        node: Arc<Node>,
    ) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            node,
            _reader_task,
            writer,
        }))
    }

    fn config(&self) -> &NodeConfig {
        &self.node.config
    }

    pub(crate) async fn send_message(&mut self, message: Vec<u8>) -> io::Result<()> {
        if message.len() > self.config().message_length_size as usize * 8 {
            // TODO: retun a nice io::Error instead
            panic!(
                "outbound message exceeded maximum message length: {} > {}",
                message.len(),
                self.config().message_length_size * 8,
            );
        }

        match (
            self.config().message_byte_order,
            self.config().message_length_size,
        ) {
            (_, 1) => self.writer.write(&[message.len() as u8]).await?,
            (BE, 2) => self.writer.write(&(message.len() as u16).to_be_bytes()).await?,
            (LE, 2) => self.writer.write(&(message.len() as u16).to_le_bytes()).await?,
            (BE, 4) => self.writer.write(&(message.len() as u32).to_be_bytes()).await?,
            (LE, 4) => self.writer.write(&(message.len() as u32).to_le_bytes()).await?,
            (BE, 8) => self.writer.write(&(message.len() as u64).to_be_bytes()).await?,
            (LE, 8) => self.writer.write(&(message.len() as u64).to_le_bytes()).await?,
            _ => unimplemented!(),
        };

        self.writer.write(&message).await?;
        self.writer.flush().await
    }
}
