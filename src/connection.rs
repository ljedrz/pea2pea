use crate::config::ByteOrder::*;
use crate::{Node, NodeConfig};

use once_cell::sync::OnceCell;
use tokio::{
    io::AsyncWriteExt,
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

    fn config(&self) -> &NodeConfig {
        &self.node.config
    }

    pub(crate) async fn send_message(&self, message: Vec<u8>) -> io::Result<()> {
        if let Some(writing_closure) = self.node.writing_closure() {
            let message = writing_closure(&message);
            let mut writer = self.writer.lock().await;
            writer.write(&message).await?;
            writer.flush().await
        } else {
            error!(parent: self.node.span(), "can't send messages! WriteProtocol is not enabled");
            Err(ErrorKind::Other.into())
        }
    }
}
