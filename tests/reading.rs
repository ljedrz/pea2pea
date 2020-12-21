use async_trait::async_trait;
use parking_lot::RwLock;
use tokio::{io::AsyncReadExt, task::JoinHandle, time::sleep};
use tracing::*;

mod common;
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

impl_read_protocol!(ArchivistNode);

// TODO: finish
