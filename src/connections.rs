//! Objects associated with connection handling.

use crate::Node;

use bytes::Bytes;
use fxhash::FxHashMap;
use parking_lot::RwLock;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpStream,
    },
    sync::mpsc::Sender,
    task::JoinHandle,
};
use tracing::*;

use std::{
    io::{self, ErrorKind},
    net::SocketAddr,
    ops::Not,
};

#[derive(Default)]
pub(crate) struct Connections(RwLock<FxHashMap<SocketAddr, Connection>>);

impl Connections {
    pub(crate) fn sender(&self, addr: SocketAddr) -> io::Result<Sender<Bytes>> {
        if let Some(conn) = self.0.read().get(&addr) {
            conn.sender()
        } else {
            Err(ErrorKind::NotConnected.into())
        }
    }

    pub(crate) fn add(&self, conn: Connection) {
        self.0.write().insert(conn.addr, conn);
    }

    pub(crate) fn senders(&self) -> io::Result<Vec<Sender<Bytes>>> {
        self.0.read().values().map(|conn| conn.sender()).collect()
    }

    pub(crate) fn is_connected(&self, addr: SocketAddr) -> bool {
        self.0.read().contains_key(&addr)
    }

    pub(crate) fn remove(&self, addr: SocketAddr) -> bool {
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

/// An object dedicated to performing reads from a connection's stream;
/// it is available only if the `Reading` protocol is enabled.
pub struct ConnectionReader {
    /// A reference to the owning node.
    pub node: Node,
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

/// An object dedicated to performing writes to a connection's stream;
/// it is available only if the `Writing` protocol is enabled.
pub struct ConnectionWriter {
    /// A reference to the owning node.
    pub node: Node,
    /// The address of the connection.
    pub addr: SocketAddr,
    /// A buffer dedicated to buffering writes to the stream.
    pub buffer: Box<[u8]>,
    /// The number of bytes from an incomplete write carried over in the buffer.
    pub carry: usize,
    /// The write half of the stream.
    pub writer: OwnedWriteHalf,
}

impl ConnectionWriter {
    /// Writes the given buffer to the stream.
    pub async fn write_all(&mut self, buffer: &[u8]) -> io::Result<()> {
        self.writer.write_all(buffer).await?;
        trace!(parent: self.node.span(), "wrote {}B to {}", buffer.len(), self.addr);

        Ok(())
    }
}

/// Keeps track of tasks that have been spawned for the purposes of a connection; it
/// also contains a sender that communicates with the `Writing` protocol handler.
pub struct Connection {
    /// A reference to the owning node.
    pub node: Node,
    /// The address of the connection.
    pub addr: SocketAddr,
    /// Kept only until the protocols are enabled (`Reading` should `take()` it).
    pub reader: Option<ConnectionReader>,
    /// Kept only until the protocols are enabled (`Writing` should `take()` it).
    pub writer: Option<ConnectionWriter>,
    /// Handles to tasks spawned by the connection.
    pub tasks: Vec<JoinHandle<()>>,
    /// Used to queue writes to the stream.
    pub outbound_message_sender: Option<Sender<Bytes>>,
    /// The connection's side in relation to the node.
    pub side: ConnectionSide,
}

impl Connection {
    /// Creates a `Connection` with placeholders for protocol-related objects.
    pub(crate) fn new(
        addr: SocketAddr,
        stream: TcpStream,
        side: ConnectionSide,
        node: &Node,
    ) -> Self {
        let (reader, writer) = stream.into_split();

        let reader = ConnectionReader {
            node: node.clone(),
            addr,
            buffer: vec![0; node.config().conn_read_buffer_size].into(),
            carry: 0,
            reader,
        };

        let writer = ConnectionWriter {
            node: node.clone(),
            addr,
            buffer: vec![0; node.config().conn_write_buffer_size].into(),
            carry: 0,
            writer,
        };

        Self {
            node: node.clone(),
            addr,
            reader: Some(reader),
            writer: Some(writer),
            side,
            tasks: Default::default(),
            outbound_message_sender: Default::default(),
        }
    }

    /// Provides mutable access to the underlying `ConnectionReader`; it should only be used in protocol definitions.
    pub fn reader(&mut self) -> &mut ConnectionReader {
        self.reader
            .as_mut()
            .expect("ConnectionReader is not available!")
    }

    /// Provides mutable access to the underlying `ConnectionWriter`; it should only be used in protocol definitions.
    pub fn writer(&mut self) -> &mut ConnectionWriter {
        self.writer
            .as_mut()
            .expect("ConnectionWriter is not available!")
    }

    /// Returns a `Sender` for outbound messages, as long as `Writing` is enabled.
    fn sender(&self) -> io::Result<Sender<Bytes>> {
        if let Some(ref sender) = self.outbound_message_sender {
            Ok(sender.clone())
        } else {
            error!(parent: self.node.span(), "can't send messages: the Writing protocol is disabled");
            Err(ErrorKind::Other.into())
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        debug!(parent: self.node.span(), "disconnecting from {}", self.addr);

        // shut the associated tasks down
        for task in &self.tasks {
            task.abort();
        }

        // if the (owning) node was not the initiator of the connection, it doesn't know the listening address
        // of the associated peer, so the related stats are unreliable; the next connection initiated by the
        // peer could be bound to an entirely different port number
        if matches!(self.side, ConnectionSide::Initiator) {
            self.node.known_peers().remove(self.addr);
        }
    }
}
