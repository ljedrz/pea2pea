//! Objects associated with connection handling.

use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    io,
    net::{IpAddr, SocketAddr},
    ops::Not,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
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

    pub(crate) fn add(&self, conn: Connection, mut guard: ConnectionGuard<'_>) -> io::Result<()> {
        // lock discipline is `limits` -> `active` everywhere; the guard's Drop (which locks
        // `limits` to clear `connecting`) runs after the `active` guard is released at scope end
        let mut active = self.active.write();
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(io::Error::other("shutting down"));
        }
        active.insert(conn.addr(), conn);
        // do NOT drop the guard or otherwise touch `limits` while `active` is held - that inverts
        // the order and deadlocks against check_and_reserve
        guard.completed = true;
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

/// A process-wide, monotonic generator of per-connection instance ids. Successive connections
/// that reuse the same [`SocketAddr`] receive distinct ids, which lets cleanup guards tell a
/// torn-down connection apart from its replacement (see [`crate::protocols::DisconnectOnDrop`]).
static SEQUENTIAL_CONN_ID: AtomicU64 = AtomicU64::new(0);

/// Created for each active connection; used by the protocols to obtain a handle for
/// reading and writing, and keeps track of tasks that have been spawned for the purposes
/// of the connection.
pub struct Connection {
    /// A process-unique id distinguishing this connection instance from any earlier or later
    /// connection that happens to reuse the same address. Used to prevent a cleanup guard
    /// belonging to a defunct connection from acting on its live successor at the same address.
    pub(crate) id: u64,
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
            id: SEQUENTIAL_CONN_ID.fetch_add(1, Ordering::Relaxed),
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

impl ConnectionLimits {
    /// Records a new pending connection: marks `addr` as connecting and charges its
    /// (canonicalized) IP against the per-IP count. Undone by clearing `connecting` and,
    /// if the connection never finalizes, [`ConnectionLimits::release_ip`].
    pub(crate) fn reserve(&mut self, addr: SocketAddr) {
        self.connecting.insert(addr);
        *self.ip_counts.entry(canonical_ip(addr)).or_insert(0) += 1;
    }

    /// Returns the number of connections (pending + established) charged to `addr`'s IP.
    pub(crate) fn ip_count(&self, addr: SocketAddr) -> usize {
        self.ip_counts
            .get(&canonical_ip(addr))
            .copied()
            .unwrap_or(0)
    }

    /// Releases one per-IP charge for `addr`, removing the bucket once it reaches zero.
    ///
    /// note: this does **not** touch `connecting`. A finalized connection clears its
    /// `connecting` entry separately (its charge having moved to the live entry), while a
    /// failed setup clears both. Because the key is derived here, every caller releases the
    /// exact bucket [`ConnectionLimits::reserve`] charged.
    pub(crate) fn release_ip(&mut self, addr: SocketAddr) {
        let ip = canonical_ip(addr);
        if let Entry::Occupied(mut e) = self.ip_counts.entry(ip) {
            debug_assert!(
                *e.get() >= 1,
                "ip_count for {ip} underflowing: release with no live reservation",
            );

            if *e.get() > 1 {
                *e.get_mut() -= 1;
            } else {
                e.remove();
            }
        }
    }
}

/// Returns the IP address used as the per-IP accounting key.
///
/// IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`, produced when a dual-stack listener bound
/// to `::` accepts IPv4 traffic without `IPV6_V6ONLY`) are folded back to their canonical
/// IPv4 form. Without this, `IpAddr::V4(a.b.c.d)` and `IpAddr::V6(::ffff:a.b.c.d)` hash as
/// distinct keys, letting a single host claim two separate `max_connections_per_ip` quotas.
const fn canonical_ip(addr: SocketAddr) -> IpAddr {
    addr.ip().to_canonical()
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
        // only release the IP charge if the connection never finalized; once completed,
        // ownership of the charge moves to the live entry and the disconnect path releases it
        if !self.completed {
            limits.release_ip(self.addr);
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

#[cfg(test)]
mod ip_count_tests {
    use super::*;

    use std::sync::atomic::AtomicBool;

    fn new_conns() -> Connections {
        Connections::new(Arc::new(AtomicBool::new(false)))
    }

    fn reserve(conns: &Connections, addr: SocketAddr) -> ConnectionGuard<'_> {
        conns.limits.lock().reserve(addr);
        ConnectionGuard {
            addr,
            connections: conns,
            completed: false,
        }
    }

    fn disconnect_decrement(conns: &Connections, addr: SocketAddr) {
        conns.limits.lock().release_ip(addr);
    }

    // direct map access keeps None = "bucket removed" for the existing assertions
    fn ip_count(conns: &Connections, addr: SocketAddr) -> Option<usize> {
        conns
            .limits
            .lock()
            .ip_counts
            .get(&canonical_ip(addr))
            .copied()
    }

    #[test]
    fn completed_guard_drop_after_concurrent_decrement_is_a_noop() {
        let conns = new_conns();
        let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();

        let mut guard = reserve(&conns, addr);
        // `add` marks the reservation completed: count ownership transfers to the
        // live entry in `active`, and `active` is unlocked before this guard drops
        guard.completed = true;

        // a concurrent disconnect removes the connection and decrements to zero
        // inside that window.
        disconnect_decrement(&conns, addr);
        assert_eq!(ip_count(&conns, addr), None);

        // must not panic and must leave the (already-correct) count untouched
        drop(guard);

        assert_eq!(ip_count(&conns, addr), None);
        assert!(!conns.limits.lock().connecting.contains(&addr)); // guard still clears `connecting`
    }

    #[test]
    fn failed_guard_drop_decrements() {
        let conns = new_conns();
        let addr: SocketAddr = "127.0.0.1:9001".parse().unwrap();

        let guard = reserve(&conns, addr); // completed stays false
        assert_eq!(ip_count(&conns, addr), Some(1));

        drop(guard);
        assert_eq!(ip_count(&conns, addr), None);
        assert!(!conns.limits.lock().connecting.contains(&addr));
    }

    #[test]
    fn per_ip_count_tracks_multiple_addrs() {
        let conns = new_conns();
        let a1: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let a2: SocketAddr = "127.0.0.1:2".parse().unwrap();

        let g1 = reserve(&conns, a1);
        let g2 = reserve(&conns, a2);
        assert_eq!(ip_count(&conns, a1), Some(2));

        drop(g1); // 2 -> 1
        assert_eq!(ip_count(&conns, a1), Some(1));

        drop(g2); // 1 -> removed
        assert_eq!(ip_count(&conns, a1), None);
    }

    #[test]
    fn ipv4_mapped_v6_shares_the_native_v4_bucket() {
        let conns = new_conns();
        let v4: SocketAddr = "127.0.0.1:8000".parse().unwrap();
        // the same host as it appears on a dual-stack `::` listener without IPV6_V6ONLY
        let mapped: SocketAddr = "[::ffff:127.0.0.1]:8001".parse().unwrap();

        // sanity: as raw IpAddr these hash as two different keys (the bug being fixed)
        assert_ne!(v4.ip(), mapped.ip());
        // ...but canonicalization collapses them to one
        assert_eq!(canonical_ip(v4), canonical_ip(mapped));

        let g1 = reserve(&conns, v4);
        let g2 = reserve(&conns, mapped);

        // both reservations land in the single canonical (IPv4) bucket, so an attacker
        // can't claim 2x the per-IP quota by switching between native and mapped forms
        assert_eq!(ip_count(&conns, v4), Some(2));
        assert_eq!(ip_count(&conns, mapped), Some(2));

        drop(g1); // 2 -> 1
        assert_eq!(ip_count(&conns, v4), Some(1));
        drop(g2); // 1 -> removed
        assert_eq!(ip_count(&conns, mapped), None);
    }
}
