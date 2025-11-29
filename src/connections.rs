//! Objects associated with connection handling.

use std::{
    collections::HashMap,
    net::SocketAddr,
    ops::Not,
    sync::{Arc, atomic::AtomicBool},
};

use parking_lot::RwLock;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    sync::oneshot,
    task::JoinHandle,
};

use crate::Stats;

#[cfg(doc)]
use crate::protocols::{Handshake, Reading, Writing};

#[derive(Default)]
pub(crate) struct Connections(pub(crate) RwLock<HashMap<SocketAddr, Connection>>);

impl Connections {
    pub(crate) fn add(&self, conn: Connection) {
        self.0.write().insert(conn.addr(), conn);
    }

    pub(crate) fn get_info(&self, addr: SocketAddr) -> Option<ConnectionInfo> {
        self.0.read().get(&addr).map(|conn| conn.info.clone())
    }

    pub(crate) fn infos(&self) -> HashMap<SocketAddr, ConnectionInfo> {
        self.0
            .read()
            .iter()
            .map(|(addr, conn)| (*addr, conn.info.clone()))
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionSide {
    /// The side that initiated the connection.
    Initiator,
    /// The side that accepted the connection.
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

/// A helper trait to facilitate trait-objectification of connection readers.
pub(crate) trait AR: AsyncRead + Unpin + Send + Sync {}
impl<T: AsyncRead + Unpin + Send + Sync> AR for T {}

/// A helper trait to facilitate trait-objectification of connection writers.
pub(crate) trait AW: AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncWrite + Unpin + Send + Sync> AW for T {}

/// Basic information related to a connection.
#[derive(Clone)]
pub struct ConnectionInfo {
    /// The address of the connection.
    addr: SocketAddr,
    /// The connection's side in relation to the node.
    side: ConnectionSide,
    /// Basic statistics related to a connection.
    stats: Arc<Stats>,
}

impl ConnectionInfo {
    /// Returns the address associated with the connection.
    #[inline]
    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Returns `Initiator` if the associated peer initiated the connection
    /// and `Responder` if the connection request was initiated by the node.
    #[inline]
    pub const fn side(&self) -> ConnectionSide {
        self.side
    }

    /// Returns basic statistics related to a connection.
    #[inline]
    pub const fn stats(&self) -> &Arc<Stats> {
        &self.stats
    }
}

/// Created for each active connection; used by the protocols to obtain a handle for
/// reading and writing, and keeps track of tasks that have been spawned for the purposes
/// of the connection.
pub struct Connection {
    /// Basic information related to a connection.
    pub(crate) info: ConnectionInfo,
    /// Available and used only in the [`Handshake`] protocol.
    pub(crate) stream: Option<TcpStream>,
    /// Available and used only in the [`Reading`] protocol.
    pub(crate) reader: Option<Box<dyn AR>>,
    /// Available and used only in the [`Writing`] protocol.
    pub(crate) writer: Option<Box<dyn AW>>,
    /// Used to notify the [`Reading`] protocol that the connection is fully ready.
    pub(crate) readiness_notifier: Option<oneshot::Sender<()>>,
    /// Prevents the OnDisconnect hook from being triggered multiple times.
    pub(crate) disconnecting: AtomicBool,
    /// Handles to tasks spawned for the connection.
    pub(crate) tasks: Vec<JoinHandle<()>>,
}

impl Connection {
    /// Creates a [`Connection`] with placeholders for protocol-related objects.
    pub(crate) fn new(addr: SocketAddr, stream: TcpStream, side: ConnectionSide) -> Self {
        Self {
            info: ConnectionInfo {
                addr,
                side,
                stats: Default::default(),
            },
            stream: Some(stream),
            reader: None,
            writer: None,
            readiness_notifier: None,
            disconnecting: Default::default(),
            tasks: Default::default(),
        }
    }

    /// Returns basic information associated with the connection.
    #[inline]
    pub const fn info(&self) -> &ConnectionInfo {
        &self.info
    }

    /// Returns the address associated with the connection.
    #[inline]
    pub const fn addr(&self) -> SocketAddr {
        self.info.addr()
    }

    /// Returns `Initiator` if the associated peer initiated the connection
    /// and `Responder` if the connection request was initiated by the node.
    #[inline]
    pub const fn side(&self) -> ConnectionSide {
        self.info.side()
    }

    /// Returns basic statistics related to a connection.
    #[inline]
    pub const fn stats(&self) -> &Arc<Stats> {
        self.info.stats()
    }
}
