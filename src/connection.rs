use crate::{Node, NodeConfig};

use once_cell::sync::OnceCell;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::tcp::{OwnedReadHalf, OwnedWriteHalf},
    sync::Mutex,
    task::JoinHandle,
};
use tracing::*;

use std::{
    io::{self, ErrorKind},
    sync::Arc,
};

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

    // FIXME: this pub is not ideal
    pub async fn read_bytes(&mut self, num: usize) -> io::Result<usize> {
        let buffer = &mut self.buffer;

        if num > buffer.len() {
            error!(parent: self.node.span(), "can' read {}B from the stream; the buffer is too small ({}B)", num, buffer.len());
            return Err(ErrorKind::Other.into());
        }

        self.reader.read_exact(&mut buffer[..num]).await
    }
}

pub struct Connection {
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

    pub(crate) async fn send_message(&self, message: Vec<u8>) -> io::Result<()> {
        if let Some(writing_closure) = self.node.writing_closure() {
            let message = writing_closure(&message);
            self.write_bytes(&message).await
        } else {
            error!(parent: self.node.span(), "can't send messages! WriteProtocol is not enabled");
            Err(ErrorKind::Other.into())
        }
    }

    // FIXME: this pub is not ideal
    pub async fn write_bytes(&self, bytes: &[u8]) -> io::Result<()> {
        let mut writer = self.writer.lock().await;
        writer.write(bytes).await?;
        writer.flush().await
    }
}
