//! Objects associated with connection handling.

use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    net::{IpAddr, SocketAddr},
    ops::Not,
    sync::{Arc, atomic::AtomicBool},
};

use parking_lot::{Mutex, RwLock};
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
pub(crate) struct Connections {
    /// The list of fully established connections.
    pub(crate) active: RwLock<HashMap<SocketAddr, Connection>>,
    /// Tracks the connection-related limits.
    pub(crate) limits: Mutex<ConnectionLimits>,
}

impl Connections {
    pub(crate) fn add(&self, conn: Connection) {
        self.active.write().insert(conn.addr(), conn);
    }

    pub(crate) fn get_info(&self, addr: SocketAddr) -> Option<ConnectionInfo> {
        self.active.read().get(&addr).map(|conn| conn.info.clone())
    }

    pub(crate) fn infos(&self) -> HashMap<SocketAddr, ConnectionInfo> {
        self.active
            .read()
            .iter()
            .map(|(addr, conn)| (*addr, conn.info.clone()))
            .collect()
    }

    pub(crate) fn is_connected(&self, addr: SocketAddr) -> bool {
        self.active.read().contains_key(&addr)
    }

    pub(crate) fn remove(&self, addr: SocketAddr) -> Option<Connection> {
        self.active.write().remove(&addr)
    }

    pub(crate) fn num_connected(&self) -> usize {
        self.active.read().len()
    }

    pub(crate) fn addrs(&self) -> Vec<SocketAddr> {
        self.active.read().keys().copied().collect()
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

impl Drop for Connection {
    fn drop(&mut self) {
        for task in self.tasks.iter().rev() {
            task.abort();
        }
    }
}

/// A helper object allowing all the connection-related limits to be modified at the same time.
#[derive(Default)]
pub(crate) struct ConnectionLimits {
    /// A list of connections that have not been finalized yet.
    pub(crate) connecting: HashSet<SocketAddr>,
    /// Tracks the number of connections (pending + established) per IP.
    pub(crate) ip_counts: HashMap<IpAddr, usize>,
}

// A helper object which ensures that a connecting entry is unique and eventually cleaned up.
pub(crate) struct ConnectionGuard<'a> {
    /// The applicable connection's address.
    pub(crate) addr: SocketAddr,
    /// A reference to the list of all connections.
    pub(crate) connections: &'a Connections,
    /// Indicates whether the connection has been finalized.
    pub(crate) completed: bool,
}

impl<'a> Drop for ConnectionGuard<'a> {
    fn drop(&mut self) {
        let mut limits = self.connections.limits.lock();
        limits.connecting.remove(&self.addr);

        // if the connection wasn't successfully established, decrement the ip count
        if !self.completed {
            if let Entry::Occupied(mut e) = limits.ip_counts.entry(self.addr.ip()) {
                if *e.get() > 1 {
                    *e.get_mut() -= 1;
                } else {
                    e.remove();
                }
            }
        }
    }
}
