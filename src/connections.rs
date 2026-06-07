//! Objects associated with connection handling.

use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    io,
    net::{IpAddr, SocketAddr},
    ops::Not,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use parking_lot::{Mutex, RwLock};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    sync::{Notify, oneshot},
    task::JoinHandle,
};
use tracing::*;

use crate::Stats;

#[cfg(doc)]
use crate::{
    Node,
    protocols::{Handshake, OnConnect, OnDisconnect, Reading, Writing},
};

pub(crate) struct Connections {
    /// The list of fully established connections.
    pub(crate) active: RwLock<HashMap<SocketAddr, Connection>>,
    /// Tracks the connection-related limits.
    pub(crate) limits: Mutex<ConnectionLimits>,
    /// The node-wide shutdown flag.
    shutting_down: Arc<AtomicBool>,
    /// Signalled each time an entry is removed from `active`. Used by
    /// `Node::shut_down` to wait for concurrent disconnects (i.e. those
    /// initiated outside `shut_down`'s own `disconnect_tasks`) to complete
    /// before the `OnDisconnect` handler is torn down.
    pub(crate) drain_notify: Notify,
}

impl Connections {
    pub(crate) fn new(shutting_down: Arc<AtomicBool>) -> Self {
        Self {
            active: Default::default(),
            limits: Default::default(),
            shutting_down,
            drain_notify: Notify::new(),
        }
    }

    pub(crate) fn add(&self, conn: Connection) -> io::Result<()> {
        let mut active = self.active.write();
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(io::Error::other("shutting down"));
        }
        active.insert(conn.addr(), conn);
        Ok(())
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
        let removed = self.active.write().remove(&addr);
        if removed.is_some() {
            // wake any task waiting for `active` to drain
            self.drain_notify.notify_waiters();
        }
        removed
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
    /// The tracing span.
    span: Span,
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

    /// Returns the tracing [`Span`] associated with the connection.
    #[inline]
    pub const fn span(&self) -> &Span {
        &self.span
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
    pub(crate) fn new(
        addr: SocketAddr,
        stream: TcpStream,
        side: ConnectionSide,
        span: Span,
    ) -> Self {
        Self {
            info: ConnectionInfo {
                span,
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

    /// Returns the tracing [`Span`] associated with the connection.
    #[inline]
    pub const fn span(&self) -> &Span {
        &self.info.span
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

        debug_assert!(
            limits.ip_counts.get(&self.addr.ip()).copied().unwrap_or(0) >= 1,
            "ip_count for {} underflowing: decrement with no live reservation",
            self.addr.ip()
        );

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

pub(crate) fn create_connection_span(addr: SocketAddr, parent: &Span) -> Span {
    macro_rules! try_span {
        ($lvl:expr) => {
            let s = span!(parent: parent, $lvl, "conn", addr = %addr);
            if !s.is_disabled() {
                return s;
            }
        };
    }

    try_span!(Level::TRACE);
    try_span!(Level::DEBUG);
    try_span!(Level::INFO);
    try_span!(Level::WARN);

    error_span!(parent: parent, "conn", addr = %addr)
}

/// Describes what triggered a disconnect, as delivered to [`OnDisconnect::on_disconnect`].
///
/// note: Handshake failures do not appear here. A failed handshake prevents the connection
/// from ever being registered, so there is no connection to disconnect.
///
/// note: When several events would race to trigger a disconnect on the same connection,
/// only the first to claim it is delivered to [`OnDisconnect::on_disconnect`]; subsequent
/// claims are silently dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DisconnectOrigin {
    /// The [`OnConnect`] task terminated abnormally before defusing its connection cleanup.
    /// In practice this almost always means the user's [`OnConnect::on_connect`] implementation
    /// panicked, and the disconnect is a side effect of that panic unwinding past the cleanup
    /// guard.
    ///
    /// note: The library also tears down the [`OnConnect`] task when [`OnConnect::ABORTABLE`]
    /// is `true` and a disconnect occurs while the hook is mid-flight, which would normally
    /// also trigger this origin - but that path always loses the race to the original cause
    /// and is therefore never delivered with this variant.
    OnConnectAbort,
    /// The reader task for this connection terminated. Typical causes are the peer closing
    /// its end of the socket, a decode error from the user-supplied [`Reading::Codec`], or
    /// no message arriving within [`Reading::IDLE_TIMEOUT_MS`]. Often (but not always)
    /// indicates a peer-side issue.
    Reading,
    /// The disconnect was initiated by [`Node::shut_down`], which tears down every active
    /// connection as part of stopping the node. Unlike [`DisconnectOrigin::User`], this
    /// signals that the entire node is going away - reconnection is not meaningful.
    Shutdown,
    /// The disconnect was explicitly requested via [`Node::disconnect`]. This is the only
    /// origin produced directly by user code; the others all reflect events the library
    /// detected internally.
    User,
    /// The writer task for this connection terminated. Typical causes are a [`Writing::TIMEOUT_MS`]
    /// timeout while flushing, an underlying socket write error, or the message channel being
    /// closed. Often correlates with the peer disappearing, but can also reflect local-side
    /// pipeline problems (slow consumer, broken pipe).
    Writing,
}
