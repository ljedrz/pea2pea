use async_trait::async_trait;
use parking_lot::RwLock;
use tokio::{io::AsyncReadExt, task::JoinHandle, time::sleep};
use tracing::*;

use pea2pea::{ConnectionReader, ContainsNode, Node, NodeConfig, ReadProtocol};

use std::{
    convert::TryInto,
    io,
    net::SocketAddr,
    ops::Deref,
    sync::Arc,
    time::{Duration, Instant},
};

struct Record {
    timestamp: Instant,
    value: String,
}

#[derive(Clone)]
struct ArchivistNode {
    node: Arc<Node>,
    archives: Arc<RwLock<Vec<Record>>>,
}

impl Deref for ArchivistNode {
    type Target = Node;

    fn deref(&self) -> &Self::Target {
        &self.node
    }
}

impl ContainsNode for ArchivistNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

#[async_trait]
impl ReadProtocol for ArchivistNode {
    async fn read_message(connection_reader: &mut ConnectionReader) -> io::Result<Vec<u8>> {
        let buffer = &mut connection_reader.buffer;
        connection_reader
            .reader
            .read_exact(&mut buffer[..2])
            .await?;
        let msg_len = u16::from_le_bytes(buffer[..2].try_into().unwrap()) as usize;
        connection_reader
            .reader
            .read_exact(&mut buffer[..msg_len])
            .await?;

        Ok(buffer[..msg_len].to_vec())
    }
}
