//! Objects associated with connection handling.

use bytes::Bytes;
use fxhash::FxHashMap;
use parking_lot::RwLock;
use tokio::{
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpStream,
    },
    sync::mpsc::Sender,
    task::JoinHandle,
};

use std::{io, net::SocketAddr, ops::Not};

#[derive(Default)]
pub(crate) struct Connections(RwLock<FxHashMap<SocketAddr, Connection>>);

impl Connections {
    pub(crate) fn sender(&self, addr: SocketAddr) -> io::Result<Sender<Bytes>> {
        if let Some(conn) = self.0.read().get(&addr) {
            conn.sender()
        } else {
            Err(io::ErrorKind::NotConnected.into())
        }
    }

    pub(crate) fn add(&self, conn: Connection) {
        self.0.write().insert(conn.addr, conn);
    }

    pub(crate) fn senders(&self) -> Vec<(Sender<Bytes>, SocketAddr)> {
        self.0
            .read()
            .values()
            .filter_map(|conn| conn.sender().ok().map(|sender| (sender, conn.addr)))
            .collect()
    }

    pub(crate) fn is_connected(&self, addr: SocketAddr) -> bool {
        self.0.read().contains_key(&addr)
    }

    pub(crate) fn remove(&self, addr: SocketAddr) -> Option<Connection> {
        self.0.write().remove(&addr)
    }

    pub(crate) fn num_connected(&self) -> usize {
        self.0.read().len()
    }

    pub(crate) fn addrs(&self) -> Vec<SocketAddr> {
        self.0.read().keys().copied().collect()
    }
}

/// Indicates who was the initiator and who was the responder when the connection was established.
#[derive(Clone, Copy, Debug)]
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

/// Keeps track of tasks that have been spawned for the purposes of a connection; it
/// also contains a sender that communicates with the `Writing` protocol handler.
pub struct Connection {
    /// The address of the connection.
    pub addr: SocketAddr,
    /// Kept only until the protocols are enabled (`Reading` should `take()` it).
    pub reader: Option<OwnedReadHalf>,
    /// Kept only until the protocols are enabled (`Writing` should `take()` it).
    pub writer: Option<OwnedWriteHalf>,
    /// Handles to tasks spawned by the connection.
    pub tasks: Vec<JoinHandle<()>>,
    /// Used to queue writes to the stream.
    pub outbound_message_sender: Option<Sender<Bytes>>,
    /// The connection's side in relation to the node.
    pub side: ConnectionSide,
}

impl Connection {
    /// Creates a `Connection` with placeholders for protocol-related objects.
    pub(crate) fn new(addr: SocketAddr, stream: TcpStream, side: ConnectionSide) -> Self {
        let (reader, writer) = stream.into_split();

        Self {
            addr,
            reader: Some(reader),
            writer: Some(writer),
            side,
            tasks: Default::default(),
            outbound_message_sender: Default::default(),
        }
    }

    /// Provides mutable access to the underlying reader; it should only be used in protocol definitions.
    pub fn reader(&mut self) -> &mut OwnedReadHalf {
        self.reader
            .as_mut()
            .expect("Connection's reader is not available!")
    }

    /// Provides mutable access to the underlying writer; it should only be used in protocol definitions.
    pub fn writer(&mut self) -> &mut OwnedWriteHalf {
        self.writer
            .as_mut()
            .expect("Connection's writer is not available!")
    }

    /// Returns a `Sender` for outbound messages, as long as `Writing` is enabled.
    fn sender(&self) -> io::Result<Sender<Bytes>> {
        if let Some(ref sender) = self.outbound_message_sender {
            Ok(sender.clone())
        } else {
            Err(io::ErrorKind::Unsupported.into())
        }
    }
}
