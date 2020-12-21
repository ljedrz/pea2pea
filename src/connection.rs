use crate::config::ByteOrder::*;
use crate::{Node, NodeConfig};

use once_cell::sync::OnceCell;
use tokio::{
    io::AsyncWriteExt,
    net::tcp::{OwnedReadHalf, OwnedWriteHalf},
    sync::Mutex,
    task::JoinHandle,
};

use std::{io, sync::Arc};

pub(crate) enum ConnectionSide {
    Initiator,
    Responder,
}

// FIXME: these pubs are not ideal
pub struct ConnectionReader {
    pub node: Arc<Node>,
    pub buffer: Vec<u8>,
    pub reader: OwnedReadHalf,
}

impl ConnectionReader {
    pub(crate) fn new(reader: OwnedReadHalf, node: Arc<Node>) -> Self {
        Self {
            buffer: vec![0; node.config.conn_read_buffer_size],
            node,
            reader,
        }
    }
}

pub(crate) struct Connection {
    node: Arc<Node>,
    pub(crate) reader_task: OnceCell<Option<JoinHandle<()>>>,
    writer: Mutex<OwnedWriteHalf>,
}

impl Connection {
    pub(crate) fn new(writer: OwnedWriteHalf, node: Arc<Node>) -> Self {
        Self {
            node,
            reader_task: Default::default(),
            writer: Mutex::new(writer),
        }
    }

    fn config(&self) -> &NodeConfig {
        &self.node.config
    }

    pub(crate) async fn send_message(&self, message: Vec<u8>) -> io::Result<()> {
        if message.len() > self.config().message_length_size as usize * 8 {
            // TODO: retun a nice io::Error instead
            panic!(
                "outbound message exceeded maximum message length: {} > {}",
                message.len(),
                self.config().message_length_size * 8,
            );
        }

        let mut writer = self.writer.lock().await;

        match (
            self.config().message_byte_order,
            self.config().message_length_size,
        ) {
            (_, 1) => writer.write(&[message.len() as u8]).await?,
            (BE, 2) => writer.write(&(message.len() as u16).to_be_bytes()).await?,
            (LE, 2) => writer.write(&(message.len() as u16).to_le_bytes()).await?,
            (BE, 4) => writer.write(&(message.len() as u32).to_be_bytes()).await?,
            (LE, 4) => writer.write(&(message.len() as u32).to_le_bytes()).await?,
            (BE, 8) => writer.write(&(message.len() as u64).to_be_bytes()).await?,
            (LE, 8) => writer.write(&(message.len() as u64).to_le_bytes()).await?,
            _ => unimplemented!(),
        };

        writer.write(&message).await?;
        writer.flush().await
    }
}
