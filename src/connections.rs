//! Objects associated with connection handling.

use crate::Node;

use bytes::Bytes;
use fxhash::FxHashMap;
use once_cell::sync::OnceCell;
use parking_lot::RwLock;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::tcp::{OwnedReadHalf, OwnedWriteHalf},
    sync::mpsc::{channel, Sender},
    task::JoinHandle,
};
use tracing::*;

use std::{
    io::{self, ErrorKind},
    net::SocketAddr,
    ops::Not,
    sync::Arc,
};

#[derive(Default)]
pub(crate) struct Connections(RwLock<FxHashMap<SocketAddr, Connection>>);

impl Connections {
    pub(crate) fn sender(&self, addr: SocketAddr) -> Option<Sender<Bytes>> {
        self.0
            .read()
            .get(&addr)
            .map(|conn| conn.message_sender.clone())
    }

    pub(crate) fn add(&self, conn: Connection) {
        self.0.write().insert(conn.addr, conn);
    }

    pub(crate) fn senders(&self) -> Vec<Sender<Bytes>> {
        self.0
            .read()
            .values()
            .map(|conn| conn.message_sender.clone())
            .collect()
    }

    pub(crate) fn is_connected(&self, addr: SocketAddr) -> bool {
        self.0.read().contains_key(&addr)
    }

    pub(crate) fn disconnect(&self, addr: SocketAddr) -> bool {
        self.0.write().remove(&addr).is_some()
    }

    pub(crate) fn num_connected(&self) -> usize {
        self.0.read().len()
    }

    pub(crate) fn addrs(&self) -> Vec<SocketAddr> {
        self.0.read().keys().copied().collect()
    }
}

/// Indicates who was the initiator and who was the responder when the connection was established.
#[derive(Clone, Copy)]
pub enum ConnectionSide {
    /// The side that initiated the connection.
    Initiator,
    /// The sider that accepted the connection.
    Responder,
}

impl Not for ConnectionSide {
    type Output = Self;

    fn not(self) -> Self::Output {
        match self {
            Self::Initiator => Self::Responder,
            Self::Responder => Self::Initiator,
        }
    }
}

/// An object dedicated to performing reads from a connection's stream.
pub struct ConnectionReader {
    /// A reference to the owning node.
    pub node: Arc<Node>,
    /// The address of the connection.
    pub addr: SocketAddr,
    /// A buffer dedicated to reading from the stream.
    pub buffer: Box<[u8]>,
    /// The number of bytes from an incomplete read carried over in the buffer.
    pub carry: usize,
    /// The read half of the stream.
    pub reader: OwnedReadHalf,
}

impl ConnectionReader {
    pub(crate) fn new(addr: SocketAddr, reader: OwnedReadHalf, node: Arc<Node>) -> Self {
        Self {
            addr,
            buffer: vec![0; node.config.conn_read_buffer_size].into(),
            carry: 0,
            reader,
            node,
        }
    }

    /// Reads as many bytes as there are queued to be read from the stream.
    pub async fn read_queued_bytes(&mut self) -> io::Result<&[u8]> {
        let len = self.reader.read(&mut self.buffer).await?;
        trace!(parent: self.node.span(), "read {}B from {}", len, self.addr);

        Ok(&self.buffer[..len])
    }

    /// Reads the specified number of bytes from the stream.
    pub async fn read_exact(&mut self, num: usize) -> io::Result<&[u8]> {
        let buffer = &mut self.buffer;

        if num > buffer.len() {
            error!(parent: self.node.span(), "can' read {}B from the stream; the buffer is too small ({}B)", num, buffer.len());
            return Err(ErrorKind::Other.into());
        }

        self.reader.read_exact(&mut buffer[..num]).await?;
        trace!(parent: self.node.span(), "read {}B from {}", num, self.addr);

        Ok(&buffer[..num])
    }
}

/// An object dedicated to performing writes to the stream, as well as keeping
/// track of tasks that have been spawned for the purposes of a connection.
pub struct Connection {
    /// A reference to the owning node.
    node: Arc<Node>,
    /// The address of the connection.
    pub(crate) addr: SocketAddr,
    /// The handle to the task performing reads from the stream.
    pub inbound_reader_task: OnceCell<JoinHandle<()>>,
    /// The handle to the task processing read messages.
    pub inbound_processing_task: OnceCell<JoinHandle<()>>,
    /// The handle to the task performing writes to the stream.
    _writer_task: JoinHandle<()>,
    /// Used to queue writes to the stream.
    pub(crate) message_sender: Sender<Bytes>,
    /// The connection's side in relation to the node.
    pub side: ConnectionSide,
}

impl Connection {
    pub(crate) fn new(
        addr: SocketAddr,
        mut writer: OwnedWriteHalf,
        node: Arc<Node>,
        side: ConnectionSide,
    ) -> Self {
        let (message_sender, mut message_receiver) =
            channel::<Bytes>(node.config.conn_outbound_queue_depth);

        let node_clone = Arc::clone(&node);
        let _writer_task = tokio::spawn(async move {
            loop {
                // TODO: when try_recv is available in tokio again (https://github.com/tokio-rs/tokio/issues/3350),
                // try adding a buffer for extra writing perf
                while let Some(msg) = message_receiver.recv().await {
                    if let Err(e) = writer.write_all(&msg).await {
                        node_clone.known_peers().register_failure(addr);
                        error!(parent: node_clone.span(), "couldn't send {}B to {}: {}", msg.len(), addr, e);
                    } else {
                        node_clone
                            .known_peers()
                            .register_sent_message(addr, msg.len());
                        node_clone.stats.register_sent_message(msg.len());
                        trace!(parent: node_clone.span(), "sent {}B to {}", msg.len(), addr);
                    }
                }
                if let Err(e) = writer.flush().await {
                    node_clone.known_peers().register_failure(addr);
                    error!(parent: node_clone.span(), "couldn't flush the stream to {}: {}", addr, e);
                }
            }
        });
        trace!(parent: node.span(), "spawned a task for writing messages to {}", addr);

        Self {
            node,
            addr,
            inbound_reader_task: Default::default(),
            inbound_processing_task: Default::default(),
            _writer_task,
            message_sender,
            side,
        }
    }

    /// Sends the given message to the peer associated with the connection.
    pub async fn send_message(&self, message: Bytes) {
        // can't recover if this happens
        self.message_sender
            .send(message)
            .await
            .expect("the connection writer task is closed");
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        debug!(parent: self.node.span(), "disconnecting from {}", self.addr);
        // if the (owning) node was not the initiator of the connection, it doesn't know the listening address
        // of the associated peer, so the related stats are unreliable; the next connection initiated by the
        // peer could be bound to an entirely different port number
        if matches!(self.side, ConnectionSide::Initiator) {
            self.node.known_peers().remove(self.addr);
        }
    }
}
