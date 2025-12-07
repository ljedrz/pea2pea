use std::{
    collections::{HashMap, hash_map::Entry},
    io::{self, ErrorKind},
    net::SocketAddr,
    ops::Deref,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering::*},
    },
    time::Duration,
};

use parking_lot::Mutex;
use tokio::{
    io::split,
    net::{TcpListener, TcpSocket, TcpStream},
    sync::{RwLock, oneshot},
    task::{self, JoinHandle, JoinSet},
    time::{sleep, timeout},
};
use tracing::*;

#[cfg(doc)]
use crate::protocols::Handshake;
use crate::{
    Config, Stats,
    connections::{
        Connection, ConnectionGuard, ConnectionInfo, ConnectionSide, Connections,
        create_connection_span,
    },
    protocols::{Protocol, Protocols},
};

// Starts the selected protocol handler for a new connection
macro_rules! enable_protocol {
    ($handler_type: ident, $node:expr, $conn: expr) => {
        if let Some(handler) = $node.protocols.$handler_type.get() {
            let (conn_returner, conn_retriever) = oneshot::channel();

            handler.trigger(($conn, conn_returner)).await;

            match conn_retriever.await {
                Ok(Ok(conn)) => conn,
                Err(_) => return Err(ErrorKind::BrokenPipe.into()),
                Ok(e) => return e,
            }
        } else {
            $conn
        }
    };
}

/// A sequential numeric identifier assigned to `Node`s that were not provided with a name.
static SEQUENTIAL_NODE_ID: AtomicUsize = AtomicUsize::new(0);

/// The types of long-running tasks supported by the Node.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum NodeTask {
    Listener,
    OnDisconnect,
    Handshake,
    OnConnect,
    Reading,
    Writing,
}

/// The central object responsible for handling connections.
///
/// note: Due to the architecture of protocol handlers capturing the node, a reference cycle exists
/// that prevents the Node from being dropped automatically. You must call [`Node::shut_down`] when
/// you are finished with a node to ensure all background tasks are aborted and sockets are closed.
#[derive(Clone)]
pub struct Node(Arc<InnerNode>);

impl Deref for Node {
    type Target = Arc<InnerNode>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// The actual node object that gets wrapped in an Arc in the Node.
#[doc(hidden)]
pub struct InnerNode {
    /// The tracing span.
    span: Span,
    /// The node's configuration.
    config: Config,
    /// The node's current listening address.
    listening_addr: RwLock<Option<SocketAddr>>,
    /// Contains objects used by the protocols implemented by the node.
    pub(crate) protocols: Protocols,
    /// Contains objects related to the node's active connections.
    pub(crate) connections: Connections,
    /// Collects statistics related to the node itself.
    stats: Stats,
    /// The node's tasks.
    pub(crate) tasks: Mutex<HashMap<NodeTask, JoinHandle<()>>>,
}

impl Node {
    /// Creates a new [`Node`] using the given [`Config`].
    pub fn new(mut config: Config) -> Self {
        // if there is no pre-configured name, assign a sequential numeric identifier
        if config.name.is_none() {
            config.name = Some(SEQUENTIAL_NODE_ID.fetch_add(1, SeqCst).to_string());
        }

        // create a tracing span containing the node's name
        let span = create_span(config.name.as_deref().unwrap());

        let node = Node(Arc::new(InnerNode {
            span,
            config,
            listening_addr: Default::default(),
            protocols: Default::default(),
            connections: Default::default(),
            stats: Default::default(),
            tasks: Default::default(),
        }));

        debug!(parent: node.span(), "the node is ready");

        node
    }

    /// Enables or disables listening for inbound connections; returns the actual bound address, which will
    /// differ from the one in [`Config::listener_addr`] if that one's port was unspecified (i.e. `0`).
    pub async fn toggle_listener(&self) -> io::Result<Option<SocketAddr>> {
        // we deliberately maintain the write guard for the entirety of this method
        let mut listening_addr = self.listening_addr.write().await;

        if let Some(old_listening_addr) = listening_addr.take() {
            let listener_task = self.tasks.lock().remove(&NodeTask::Listener).unwrap(); // can't fail
            listener_task.abort();
            trace!(parent: self.span(), "aborted the listening task");
            debug!(parent: self.span(), "no longer listening on {old_listening_addr}");
            *listening_addr = None;

            Ok(None)
        } else {
            let listener_addr = self.config().listener_addr.ok_or_else(|| {
                error!(parent: self.span(), "the listener was toggled on, but Config::listener_addr is not set");
                ErrorKind::AddrNotAvailable
            })?;
            trace!(parent: self.span(), "attempting to listen on {listener_addr}");
            let listener = TcpListener::bind(listener_addr).await?;
            let port = listener.local_addr()?.port(); // discover the port if it was unspecified
            let new_listening_addr = (listener_addr.ip(), port).into();

            // start listening
            self.start_listening(listener).await;
            debug!(parent: self.span(), "listening on {new_listening_addr}");

            // update the node's listening address
            *listening_addr = Some(new_listening_addr);

            Ok(Some(new_listening_addr))
        }
    }

    /// Spawn a task responsible for listening for inbound connections.
    async fn start_listening(&self, listener: TcpListener) {
        // use a channel to know when the listening task is ready
        let (tx, rx) = oneshot::channel();

        let node = self.clone();
        let listening_task = tokio::spawn(async move {
            trace!(parent: node.span(), "spawned the listening task");
            if tx.send(()).is_err() {
                error!(parent: node.span(), "listener setup interrupted; shutting down the listening task");
                return;
            }

            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        // handle connection requests asynchronously
                        let node = node.clone();
                        tokio::spawn(async move {
                            node.handle_connection_request(stream, addr).await.inspect_err(|e|
                                match e.kind() {
                                    ErrorKind::QuotaExceeded | ErrorKind::AlreadyExists => {
                                        debug!(parent: node.span(), "rejecting connection from {addr}: {e}");
                                    }
                                    _ => {
                                        error!(parent: node.span(), "couldn't accept a connection from {addr}: {e}");
                                    }
                                }
                            )
                        });
                    }
                    Err(e) => {
                        error!(parent: node.span(), "couldn't accept a connection: {e}");
                        // if we ran out of FDs, sleep to avoid spinning 100% CPU
                        // while waiting for a slot to free up
                        sleep(Duration::from_millis(500)).await;
                    }
                }
            }
        });

        self.tasks.lock().insert(NodeTask::Listener, listening_task);
        let _ = rx.await;
    }

    /// Processes a single inbound connection request. Only used in [`Node::start_listening`].
    async fn handle_connection_request(
        &self,
        stream: TcpStream,
        addr: SocketAddr,
    ) -> io::Result<()> {
        // check connection limits and set up a connection guard
        let guard = self.check_and_reserve(addr)?;

        // finalize the connection
        self.adapt_stream(stream, addr, ConnectionSide::Responder, guard)
            .await
    }

    /// Returns the name assigned to the node.
    #[inline]
    pub fn name(&self) -> &str {
        // safe; can be set as None in Config, but receives a default value on Node creation
        self.config.name.as_deref().unwrap()
    }

    /// Returns a reference to the node's config.
    #[inline]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Returns a reference to the node's stats.
    #[inline]
    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Returns the tracing [`Span`] associated with the node.
    #[inline]
    pub fn span(&self) -> &Span {
        &self.span
    }

    /// Returns the node's current listening address; returns an error if the node was configured
    /// to not listen for inbound connections or if the listener is currently disabled.
    pub async fn listening_addr(&self) -> io::Result<SocketAddr> {
        self.listening_addr
            .read()
            .await
            .as_ref()
            .copied()
            .ok_or_else(|| ErrorKind::AddrNotAvailable.into())
    }

    /// Enable the applicable protocols for a new connection.
    async fn enable_protocols(&self, conn: Connection) -> io::Result<Connection> {
        let mut conn = enable_protocol!(handshake, self, conn);

        // split the stream after the handshake (if not done before)
        if let Some(stream) = conn.stream.take() {
            let (reader, writer) = split(stream);
            conn.reader = Some(Box::new(reader));
            conn.writer = Some(Box::new(writer));
        }

        let conn = enable_protocol!(reading, self, conn);
        let conn = enable_protocol!(writing, self, conn);

        Ok(conn)
    }

    /// Prepares the freshly acquired connection to handle the protocols the Node implements.
    async fn adapt_stream(
        &self,
        stream: TcpStream,
        peer_addr: SocketAddr,
        own_side: ConnectionSide,
        mut guard: ConnectionGuard<'_>,
    ) -> io::Result<()> {
        let conn_span = create_connection_span(peer_addr, self.span());
        debug!(parent: &conn_span, "establishing connection as the {own_side:?}");

        // register the port seen by the peer
        if own_side == ConnectionSide::Initiator {
            if let Ok(addr) = stream.local_addr() {
                trace!(parent: &conn_span, "the peer is connected on port {}", addr.port());
            } else {
                warn!(parent: &conn_span, "couldn't determine the peer-side port");
            }
        }

        let connection = Connection::new(peer_addr, stream, !own_side, conn_span.clone());

        // enact the enabled protocols
        let mut connection = self.enable_protocols(connection).await?;

        // if Reading is enabled, we'll notify the related task when the connection is fully ready
        let conn_ready_tx = connection.readiness_notifier.take();

        // connecting -> connected
        self.connections.add(connection);
        guard.completed = true;
        drop(guard);

        // send the aforementioned notification so that reading from the socket can commence
        if let Some(tx) = conn_ready_tx {
            let _ = tx.send(());
        }

        debug!(parent: &conn_span, "fully connected");

        // if enabled, enact OnConnect
        if let Some(handler) = self.protocols.on_connect.get() {
            trace!(parent: &conn_span, "executing OnConnect logic...");
            let (sender, receiver) = oneshot::channel();
            handler.trigger((peer_addr, sender)).await;

            // receive the handle for the running task
            if let Ok(handle) = receiver.await {
                if let Some(conn) = self.connections.active.write().get_mut(&peer_addr) {
                    // add the task to the connection so it gets aborted in case of a disconnect
                    conn.tasks.push(handle);
                } else {
                    // the connection has just been terminated; abort the OnConnect work
                    handle.abort();
                }
            }
        }

        Ok(())
    }

    // A helper method to facilitate a common potential disconnect at the callsite.
    async fn create_stream(
        &self,
        addr: SocketAddr,
        socket: Option<TcpSocket>,
    ) -> io::Result<TcpStream> {
        match timeout(
            Duration::from_millis(self.config().connection_timeout_ms.into()),
            self.create_stream_inner(addr, socket),
        )
        .await
        {
            Ok(Ok(stream)) => Ok(stream),
            Ok(err) => err,
            Err(err) => Err(io::Error::new(ErrorKind::TimedOut, err)),
        }
    }

    /// A wrapper method for greater readability.
    async fn create_stream_inner(
        &self,
        addr: SocketAddr,
        socket: Option<TcpSocket>,
    ) -> io::Result<TcpStream> {
        if let Some(socket) = socket {
            socket.connect(addr).await
        } else {
            TcpStream::connect(addr).await
        }
    }

    /// Connects to the provided `SocketAddr`.
    ///
    /// note: `pea2pea` identifies connections by their socket address (IP + port). If Node A
    /// connects to Node B, and Node B simultaneously connects to Node A, the library considers
    /// these two distinct connections (one outgoing, one incoming). To ensure a single logical
    /// connection per peer, you must implement a tie-breaking mechanism in your application logic
    /// in the [`Handshake`] protocol.
    pub async fn connect(&self, addr: SocketAddr) -> io::Result<()> {
        self.connect_inner(addr, None)
            .await
            .inspect_err(|e| error!(parent: self.span(), "couldn't connect to {addr}: {e}"))
    }

    /// Connects to a `SocketAddr` using the provided `TcpSocket`.
    pub async fn connect_using_socket(
        &self,
        addr: SocketAddr,
        socket: TcpSocket,
    ) -> io::Result<()> {
        self.connect_inner(addr, Some(socket))
            .await
            .inspect_err(|e| error!(parent: self.span(), "couldn't connect to {addr}: {e}"))
    }

    /// Connects to the provided `SocketAddr` using an optional `TcpSocket`.
    async fn connect_inner(&self, addr: SocketAddr, socket: Option<TcpSocket>) -> io::Result<()> {
        // a simple self-connect attempt check
        if let Ok(listening_addr) = self.listening_addr().await {
            if addr == listening_addr
                || addr.ip().is_loopback() && addr.port() == listening_addr.port()
            {
                return Err(io::Error::new(
                    ErrorKind::AddrInUse,
                    "can't connect to node's own listening address ({addr})",
                ));
            }
        }

        // make sure the address is not already connected to, unless
        // duplicate connections are permitted in the config
        if !self.config.allow_duplicate_connections && self.connections.is_connected(addr) {
            return Err(io::Error::new(
                ErrorKind::AlreadyExists,
                "already connected to {addr}",
            ));
        }

        // attempt to reserve a connection slot atomically
        let guard = self.check_and_reserve(addr)?;

        // attempt to physically connect to the specified address
        let stream = self.create_stream(addr, socket).await?;

        // attempt to finalize the connection
        self.adapt_stream(stream, addr, ConnectionSide::Initiator, guard)
            .await
    }

    /// Disconnects from the provided `SocketAddr`; returns `true` if an actual disconnect took place.
    pub async fn disconnect(&self, addr: SocketAddr) -> bool {
        // claim the disconnect to avoid duplicate executions, or return early if already claimed
        if let Some(conn) = self.connections.active.read().get(&addr) {
            if conn.disconnecting.swap(true, Relaxed) {
                // valid connection, but someone else is already disconnecting it
                return false;
            }
        } else {
            // not connected
            return false;
        };

        let conn_span = create_connection_span(addr, self.span());
        debug!(parent: &conn_span, "disconnecting...");

        // if the OnDisconnect protocol is enabled, trigger it
        if let Some(handler) = self.protocols.on_disconnect.get() {
            trace!(parent: &conn_span, "executing OnDisconnect logic...");
            let (sender, receiver) = oneshot::channel();
            handler.trigger((addr, sender)).await;
            if let Ok((handle, waiter)) = receiver.await {
                // register the associated task with the connection, in case
                // it gets terminated before its completion
                if let Some(conn) = self.connections.active.write().get_mut(&addr) {
                    conn.tasks.push(handle);
                }
                // wait for the OnDisconnect protocol to perform its specified actions
                // time out, or even panic - we're already disconnecting, so ignore the
                // result
                let _ = waiter.await;
            }
        }

        // ensure that any OnDisconnect-related writes can conclude
        if let Some(writing) = self.protocols.writing.get() {
            // remove the connection's message sender so that
            // the associated loop can exit organically
            writing.senders.write().remove(&addr);

            // give the Writing task a chance to process it
            // and flush any final messages to the kernel
            task::yield_now().await;
        }

        // the connection can now be "physically" removed
        let _ = self.connections.remove(addr);

        // decrement the per-IP connection count
        if let Entry::Occupied(mut e) = self.connections.limits.lock().ip_counts.entry(addr.ip()) {
            if *e.get() > 1 {
                *e.get_mut() -= 1;
            } else {
                e.remove();
            }
        }

        debug!(parent: &conn_span, "fully disconnected");

        true
    }

    /// Returns a list containing addresses of active connections.
    pub fn connected_addrs(&self) -> Vec<SocketAddr> {
        self.connections.addrs()
    }

    /// Checks whether the provided address is connected.
    pub fn is_connected(&self, addr: SocketAddr) -> bool {
        self.connections.is_connected(addr)
    }

    /// Checks if the node is currently setting up a connection with the provided address.
    pub fn is_connecting(&self, addr: SocketAddr) -> bool {
        self.connections.limits.lock().connecting.contains(&addr)
    }

    /// Returns the number of active connections.
    pub fn num_connected(&self) -> usize {
        self.connections.num_connected()
    }

    /// Returns the number of connections that are currently being set up.
    pub fn num_connecting(&self) -> usize {
        self.connections.limits.lock().connecting.len()
    }

    /// Returns basic information related to a connection.
    pub fn connection_info(&self, addr: SocketAddr) -> Option<ConnectionInfo> {
        self.connections.get_info(addr)
    }

    /// Returns a list of all active connections and their basic information.
    pub fn connection_infos(&self) -> HashMap<SocketAddr, ConnectionInfo> {
        self.connections.infos()
    }

    /// Atomically checks connection limits and reserves a slot if available.
    fn check_and_reserve(&self, addr: SocketAddr) -> io::Result<ConnectionGuard<'_>> {
        // this lock is held for the duration of the check to prevent races
        let mut limits = self.connections.limits.lock();

        // check the per-IP limit first
        let ip = addr.ip();
        let num_ip_conns = *limits.ip_counts.get(&ip).unwrap_or(&0);
        let per_ip_limit = self.config.max_connections_per_ip as usize;
        if num_ip_conns >= per_ip_limit {
            return Err(io::Error::new(
                ErrorKind::QuotaExceeded,
                "maximum number ({per_ip_limit}) of per-IP connections reached with {ip}",
            ));
        }

        // check the global connecting count limit
        let num_connecting = limits.connecting.len();
        let connecting_limit = self.config.max_connecting as usize;
        if num_connecting >= connecting_limit {
            return Err(io::Error::new(
                ErrorKind::QuotaExceeded,
                "maximum number ({connecting_limit}) of pending connections reached",
            ));
        }

        // check the global connection count limit
        let num_connected = self.connections.num_connected();
        let connection_limit = self.config.max_connections as usize;
        if num_connected + num_connecting >= connection_limit {
            return Err(io::Error::new(
                ErrorKind::QuotaExceeded,
                "maximum number ({connection_limit}) of connections reached",
            ));
        }

        // check if already connecting (duplicate connection attempt from same node)
        if limits.connecting.contains(&addr) {
            return Err(io::Error::new(
                ErrorKind::AlreadyExists,
                "already connecting to {addr}",
            ));
        }

        // reserve a connecting slot
        *limits.ip_counts.entry(ip).or_insert(0) += 1;
        limits.connecting.insert(addr);

        Ok(ConnectionGuard {
            addr,
            connections: &self.connections,
            completed: false,
        })
    }

    /// Gracefully shuts the node down.
    pub async fn shut_down(&self) {
        debug!(parent: self.span(), "shutting down");

        let mut tasks = std::mem::take(&mut *self.tasks.lock());

        // abort the listening task first (if it exists)
        if let Some(listening_task) = tasks.remove(&NodeTask::Listener) {
            listening_task.abort();
        }

        // disconnect from all the peers
        let mut disconnect_tasks = JoinSet::new();
        for addr in self.connected_addrs() {
            let node = self.clone();
            disconnect_tasks.spawn(async move {
                node.disconnect(addr).await;
            });
        }
        while disconnect_tasks.join_next().await.is_some() {}

        // abort the remaining tasks, which should now be inert
        for handle in tasks.into_values() {
            handle.abort();
        }
    }
}

/// Creates the node's tracing span based on its name.
fn create_span(node_name: &str) -> Span {
    macro_rules! try_span {
        ($lvl:expr) => {
            let s = span!($lvl, "node", name = node_name);
            if !s.is_disabled() {
                return s;
            }
        };
    }

    try_span!(Level::TRACE);
    try_span!(Level::DEBUG);
    try_span!(Level::INFO);
    try_span!(Level::WARN);

    error_span!("node", name = node_name)
}
