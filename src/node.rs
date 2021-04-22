use crate::{
    connections::{Connection, ConnectionSide, Connections},
    protocols::{ProtocolHandler, Protocols},
    KnownPeers, NodeConfig, NodeStats,
};

use bytes::Bytes;
use fxhash::FxHashSet;
use parking_lot::Mutex;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
};
use tracing::*;

use std::{
    io,
    net::SocketAddr,
    ops::Deref,
    sync::{
        atomic::{AtomicUsize, Ordering::*},
        Arc,
    },
};

macro_rules! enable_protocol {
    ($protocol_name: expr, $handler_type: ident, $node:expr, $conn: expr) => {
        if let Some(ref handler) = $node.protocols.$handler_type.get() {
            let (conn_returner, conn_retriever) = oneshot::channel();

            handler.send(($conn, conn_returner)).await;

            match conn_retriever.await {
                Ok(Ok(conn)) => conn,
                Err(_) => unreachable!(), // protocol's task is down! can't recover
                Ok(e) => return e,
            }
        } else {
            $conn
        }
    };
}

// A seuential numeric identifier assigned to `Node`s that were not provided with a name.
static SEQUENTIAL_NODE_ID: AtomicUsize = AtomicUsize::new(0);

/// The central object responsible for handling all the connections.
#[derive(Clone)]
pub struct Node(Arc<InnerNode>);

impl Deref for Node {
    type Target = Arc<InnerNode>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[doc(hidden)]
pub struct InnerNode {
    /// The tracing span.
    span: Span,
    /// The node's configuration.
    config: NodeConfig,
    /// The node's listening address.
    listening_addr: SocketAddr,
    /// Contains objects used by the protocols implemented by the node.
    protocols: Protocols,
    /// A list of connections that have not been finalized yet.
    connecting: Mutex<FxHashSet<SocketAddr>>,
    /// Contains objects related to the node's active connections.
    connections: Connections,
    /// Collects statistics related to the node's peers.
    known_peers: KnownPeers,
    /// Collects statistics related to the node itself.
    stats: NodeStats,
    /// The node's tasks.
    pub(crate) tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Node {
    /// Creates a new `Node` optionally using a given `NodeConfig`.
    pub async fn new(config: Option<NodeConfig>) -> io::Result<Self> {
        let mut config = config.unwrap_or_default();

        // if there is no pre-configured name, assign a sequential numeric identifier
        if config.name.is_none() {
            config.name = Some(SEQUENTIAL_NODE_ID.fetch_add(1, SeqCst).to_string());
        }

        // create a tracing span containing the node's name
        let span = create_span(config.name.as_deref().unwrap());

        // procure a listening address
        let listener_ip = config.listener_ip;
        let listener = if let Some(port) = config.desired_listening_port {
            let desired_listening_addr = SocketAddr::new(listener_ip, port);
            match TcpListener::bind(desired_listening_addr).await {
                Ok(listener) => listener,
                Err(e) => {
                    if config.allow_random_port {
                        warn!(parent: span.clone(), "trying any port, the desired one is unavailable: {}", e);
                        let random_available_addr = SocketAddr::new(listener_ip, 0);
                        TcpListener::bind(random_available_addr).await?
                    } else {
                        error!(parent: span.clone(), "the desired port is unavailable: {}", e);
                        return Err(e);
                    }
                }
            }
        } else if config.allow_random_port {
            let random_available_addr = SocketAddr::new(listener_ip, 0);
            TcpListener::bind(random_available_addr).await?
        } else {
            panic!("you must either provide a desired port or allow a random port to be chosen");
        };

        let listening_addr = listener.local_addr()?;

        let node = Node(Arc::new(InnerNode {
            span,
            config,
            listening_addr,
            protocols: Default::default(),
            connecting: Default::default(),
            connections: Default::default(),
            known_peers: Default::default(),
            stats: Default::default(),
            tasks: Default::default(),
        }));

        let node_clone = node.clone();
        let listening_task = tokio::spawn(async move {
            trace!(parent: node_clone.span(), "spawned the listening task");
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        debug!(parent: node_clone.span(), "tentatively accepted a connection from {}", addr);

                        if !node_clone.can_add_connection() {
                            debug!(parent: node_clone.span(), "rejecting the connection from {}", addr);
                            continue;
                        }

                        if let Err(e) = node_clone
                            .adapt_stream(stream, addr, ConnectionSide::Responder)
                            .await
                        {
                            node_clone.known_peers().register_failure(addr);
                            error!(parent: node_clone.span(), "couldn't accept a connection: {}", e);
                        }
                    }
                    Err(e) => {
                        error!(parent: node_clone.span(), "couldn't accept a connection: {}", e);
                    }
                }
            }
        });
        node.tasks.lock().push(listening_task);

        debug!(parent: node.span(), "the node is ready; listening on {}", listening_addr);

        Ok(node)
    }

    /// Returns the name assigned to the node.
    pub fn name(&self) -> &str {
        // safe; can be set as None in NodeConfig, but receives a default value on Node creation
        self.config.name.as_deref().unwrap()
    }

    /// Returns a reference to the node's config.
    pub fn config(&self) -> &NodeConfig {
        &self.config
    }

    /// Returns a reference to the node's stats.
    pub fn stats(&self) -> &NodeStats {
        &self.stats
    }

    /// Returns the tracing `Span` associated with the node.
    pub fn span(&self) -> &Span {
        &self.span
    }

    /// Returns the node's listening address.
    pub fn listening_addr(&self) -> SocketAddr {
        self.listening_addr
    }

    async fn enable_protocols(&self, conn: Connection) -> io::Result<Connection> {
        let conn = enable_protocol!("HandshakeProtocol", handshake_handler, self, conn);
        let conn = enable_protocol!("ReadingProtocol", reading_handler, self, conn);
        let conn = enable_protocol!("WritingProtocol", writing_handler, self, conn);

        Ok(conn)
    }

    /// Prepares the freshly acquired connection to handle the protocols the Node implements.
    async fn adapt_stream(
        &self,
        stream: TcpStream,
        peer_addr: SocketAddr,
        own_side: ConnectionSide,
    ) -> io::Result<()> {
        self.known_peers.add(peer_addr);

        // register the port seen by the peer
        if let ConnectionSide::Initiator = own_side {
            if let Ok(addr) = stream.local_addr() {
                debug!(
                    parent: self.span(), "establishing connection with {}; the peer is connected on port {}",
                    peer_addr, addr.port()
                );
            } else {
                warn!(parent: self.span(), "couldn't determine the peer's port");
            }
        }

        let connection = Connection::new(peer_addr, stream, !own_side, self);

        // enact the enabled protocols
        let mut connection = self.enable_protocols(connection).await?;

        // the protocols are responsible for doing reads and writes; ensure that the Connection object
        // is not capable of performing them if the protocols haven't been enabled.
        connection.reader = None;
        connection.writer = None;

        self.connections.add(connection);
        self.known_peers.register_connection(peer_addr);

        Ok(())
    }

    /// Connects to the provided `SocketAddr`.
    pub async fn connect(&self, addr: SocketAddr) -> io::Result<()> {
        if addr == self.listening_addr()
            || addr.ip().is_loopback() && addr.port() == self.listening_addr().port()
        {
            error!(parent: self.span(), "can't connect to node's own listening address ({})", addr);
            return Err(io::ErrorKind::AddrInUse.into());
        }

        if !self.can_add_connection() {
            error!(parent: self.span(), "too many connections; refusing to connect to {}", addr);
            return Err(io::ErrorKind::Other.into());
        }

        if self.connections.is_connected(addr) {
            warn!(parent: self.span(), "already connected to {}", addr);
            return Err(io::ErrorKind::AlreadyExists.into());
        }

        if !self.connecting.lock().insert(addr) {
            warn!(parent: self.span(), "already connecting to {}", addr);
            return Err(io::ErrorKind::AlreadyExists.into());
        }

        let stream = TcpStream::connect(addr).await.map_err(|e| {
            self.connecting.lock().remove(&addr);
            e
        })?;

        let ret = self
            .adapt_stream(stream, addr, ConnectionSide::Initiator)
            .await;

        if let Err(ref e) = ret {
            self.known_peers().register_failure(addr);
            error!(parent: self.span(), "couldn't initiate a connection with {}: {}", addr, e);
        }

        self.connecting.lock().remove(&addr);

        ret
    }

    /// Disconnects from the provided `SocketAddr`.
    pub fn disconnect(&self, addr: SocketAddr) -> bool {
        let disconnected = self.connections.remove(addr);

        if disconnected {
            info!(parent: self.span(), "disconnected from {}", addr);
        } else {
            warn!(parent: self.span(), "wasn't connected to {}", addr);
        }

        disconnected
    }

    /// Sends the provided message to the specified `SocketAddr`, as long as the `Writing` protocol is enabled.
    pub async fn send_direct_message(&self, addr: SocketAddr, message: Bytes) -> io::Result<()> {
        self.connections
            .sender(addr)?
            .send(message)
            .await
            .map_err(|_| io::ErrorKind::NotConnected.into()) // an error here means the connection was shut down
    }

    /// Broadcasts the provided message to all peers, as long as the `Writing` protocol is enabled.
    pub async fn send_broadcast(&self, message: Bytes) -> io::Result<()> {
        for message_sender in self.connections.senders()? {
            // an error means the connection is shutting down, which is already reported in logs
            let _ = message_sender.send(message.clone()).await;
        }

        Ok(())
    }

    /// Returns a list containing addresses of active connections.
    pub fn connected_addrs(&self) -> Vec<SocketAddr> {
        self.connections.addrs()
    }

    /// Returns a reference to the collection of statistics of node's known peers.
    pub fn known_peers(&self) -> &KnownPeers {
        &self.known_peers
    }

    /// Checks whether the provided address is connected.
    pub fn is_connected(&self, addr: SocketAddr) -> bool {
        self.connections.is_connected(addr)
    }

    /// Returns the number of active connections.
    pub fn num_connected(&self) -> usize {
        self.connections.num_connected()
    }

    /// Checks whether the `Node` can handle an additional connection.
    fn can_add_connection(&self) -> bool {
        let num_connected = self.num_connected();
        let limit = self.config.max_connections as usize;
        if num_connected >= limit || num_connected + self.connecting.lock().len() >= limit {
            warn!(parent: self.span(), "maximum number of connections ({}) reached", limit);
            false
        } else {
            true
        }
    }

    /// Sets up the handshake handler, as part of the `Handshaking` protocol.
    pub fn set_handshake_handler(&self, handler: ProtocolHandler) {
        if self.protocols.handshake_handler.set(handler).is_err() {
            panic!("the handshake_handler field was set more than once!");
        }
    }

    /// Sets up the reading handler, as part of enabling the `Reading` protocol.
    pub fn set_reading_handler(&self, handler: ProtocolHandler) {
        if self.protocols.reading_handler.set(handler).is_err() {
            panic!("the reading_handler field was set more than once!");
        }
    }

    /// Sets up the writing handler, as part of enabling the `Writing` protocol.
    pub fn set_writing_handler(&self, handler: ProtocolHandler) {
        if self.protocols.writing_handler.set(handler).is_err() {
            panic!("the writing_handler field was set more than once!");
        }
    }

    /// Gracefully shuts the node down.
    pub fn shut_down(&self) {
        debug!(parent: self.span(), "shutting down");

        let mut tasks = std::mem::take(&mut *self.tasks.lock()).into_iter();
        if let Some(listening_task) = tasks.next() {
            listening_task.abort(); // abort the listening task first
        }

        for addr in self.connected_addrs() {
            self.disconnect(addr);
        }

        for handle in tasks {
            handle.abort();
        }
    }
}

// FIXME: this can probably be done more elegantly
/// Creates the node's tracing span based on its name.
fn create_span(node_name: &str) -> Span {
    let mut span = trace_span!("node", name = node_name);
    if !span.is_disabled() {
        return span;
    } else {
        span = debug_span!("node", name = node_name);
    }
    if !span.is_disabled() {
        return span;
    } else {
        span = info_span!("node", name = node_name);
    }
    if !span.is_disabled() {
        return span;
    } else {
        span = warn_span!("node", name = node_name);
    }
    if !span.is_disabled() {
        span
    } else {
        error_span!("node", name = node_name)
    }
}
