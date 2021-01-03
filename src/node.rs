use crate::connections::{Connection, ConnectionSide, Connections};
use crate::protocols::{HandshakeHandler, Protocols, ReadingHandler, WritingHandler};
use crate::*;

use bytes::Bytes;
use once_cell::sync::OnceCell;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
};
use tracing::*;

use std::{
    io::{self, ErrorKind},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        atomic::{AtomicUsize, Ordering::*},
        Arc,
    },
};

macro_rules! enable_protocol {
    ($protocol_name: expr, $handler_type: ident, $node:expr, $conn: expr) => {
        if let Some(ref handler) = $node.$handler_type() {
            let (conn_returner, conn_retriever) = oneshot::channel();

            handler.send(($conn, conn_returner)).await;

            match conn_retriever.await {
                Ok(Ok(conn)) => conn,
                _ => {
                    return Err(io::Error::new(
                        ErrorKind::Other,
                        format!("{} failed", $protocol_name),
                    ));
                }
            }
        } else {
            $conn
        }
    };
}

// A seuential numeric identifier assigned to `Node`s that were not provided with a name.
static SEQUENTIAL_NODE_ID: AtomicUsize = AtomicUsize::new(0);

/// The central object responsible for handling all the connections.
pub struct Node {
    /// The tracing span.
    span: Span,
    /// The node's configuration.
    pub config: NodeConfig,
    /// The node's listening address.
    pub listening_addr: SocketAddr,
    /// Contains objects used by the protocols implemented by the node.
    protocols: Protocols,
    /// Contains objects related to the node's active connections.
    connections: Connections,
    /// Collects statistics related to the node's peers.
    known_peers: KnownPeers,
    /// Collects statistics related to the node itself.
    pub stats: NodeStats,
    /// The node's listening task.
    listening_task: OnceCell<JoinHandle<()>>,
}

impl Node {
    /// Returns a `Node` wrapped in an `Arc`, so it can be cloned around.
    pub async fn new(config: Option<NodeConfig>) -> io::Result<Arc<Self>> {
        let local_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let mut config = config.unwrap_or_default();

        // if there is no pre-configured name, assign a sequential numeric identifier
        if config.name.is_none() {
            config.name = Some(SEQUENTIAL_NODE_ID.fetch_add(1, SeqCst).to_string());
        }

        // create a tracing span containing the node's name
        let span = create_span(config.name.as_deref().unwrap());

        // procure a listening address
        let listener = if let Some(port) = config.desired_listening_port {
            let desired_listening_addr = SocketAddr::new(local_ip, port);
            match TcpListener::bind(desired_listening_addr).await {
                Ok(listener) => listener,
                Err(e) => {
                    if config.allow_random_port {
                        warn!(parent: span.clone(), "trying any port, the desired one is unavailable: {}", e);
                        let random_available_addr = SocketAddr::new(local_ip, 0);
                        TcpListener::bind(random_available_addr).await?
                    } else {
                        error!(parent: span.clone(), "the desired port is unavailable: {}", e);
                        return Err(e);
                    }
                }
            }
        } else if config.allow_random_port {
            let random_available_addr = SocketAddr::new(local_ip, 0);
            TcpListener::bind(random_available_addr).await?
        } else {
            panic!("you must either provide a desired port or allow a random port to be chosen");
        };

        let listening_addr = listener.local_addr()?;

        let node = Arc::new(Self {
            span,
            config,
            listening_addr,
            protocols: Default::default(),
            connections: Default::default(),
            known_peers: Default::default(),
            stats: Default::default(),
            listening_task: Default::default(),
        });

        let node_clone = Arc::clone(&node);
        let listening_task = tokio::spawn(async move {
            trace!(parent: node_clone.span(), "spawned the listening task");
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
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

        node.listening_task.set(listening_task).unwrap();

        debug!(parent: node.span(), "the node is ready; listening on {}", listening_addr);

        Ok(node)
    }

    /// Returns the name assigned to the node.
    pub fn name(&self) -> &str {
        // safe; can be set as None in NodeConfig, but receives a default value on Node creation
        self.config.name.as_deref().unwrap()
    }

    /// Returns the tracing `Span` associated with the node.
    pub fn span(&self) -> &Span {
        &self.span
    }

    /// Prepares the freshly acquired connection to handle the protocols the Node implements.
    async fn adapt_stream(
        self: &Arc<Self>,
        stream: TcpStream,
        peer_addr: SocketAddr,
        own_side: ConnectionSide,
    ) -> io::Result<()> {
        self.stats.register_connection();

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
        let connection = enable_protocol!("HandshakeProtocol", handshake_handler, self, connection);
        let connection = enable_protocol!("ReadingProtocol", reading_handler, self, connection);
        let mut connection = enable_protocol!("WritingProtocol", writing_handler, self, connection);

        // the protocols are responsible for doing reads and writes; ensure that the Connection object
        // is not capable of performing them if the protocols haven't been enabled.
        connection.reader = None;
        connection.writer = None;

        self.connections.add(connection);
        self.known_peers.register_connection(peer_addr);

        Ok(())
    }

    /// Connects to the provided `SocketAddr`.
    pub async fn connect(self: &Arc<Self>, addr: SocketAddr) -> io::Result<()> {
        if self.connections.is_connected(addr) {
            warn!(parent: self.span(), "already connected to {}", addr);
            return Err(ErrorKind::Other.into());
        }

        let stream = TcpStream::connect(addr).await?;
        self.adapt_stream(stream, addr, ConnectionSide::Initiator)
            .await
            .map_err(|e| {
                self.known_peers().register_failure(addr);
                error!(parent: self.span(), "couldn't initiate a connection with {}: {}", addr, e);
                e
            })
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
            .expect("the connection writer task is closed"); // can't recover if this happens

        Ok(())
    }

    /// Broadcasts the provided message to all peers, as long as the `Writing` protocol is enabled.
    pub async fn send_broadcast(&self, message: Bytes) -> io::Result<()> {
        for message_sender in self.connections.senders()? {
            message_sender
                .send(message.clone())
                .await
                .expect("the connection writer task is closed"); // can't recover if this happens
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

    /// Returns a reference to the handshake handler, if the `Handshaking` protocol is enabled.
    fn handshake_handler(&self) -> Option<&HandshakeHandler> {
        self.protocols.handshake_handler.get()
    }

    /// Returns a reference to the reading handler, if the `Reading` protocol is enabled.
    pub fn reading_handler(&self) -> Option<&ReadingHandler> {
        self.protocols.reading_handler.get()
    }

    /// Returns a reference to the writing handler, if the `Writing` protocol is enabled.
    pub fn writing_handler(&self) -> Option<&WritingHandler> {
        self.protocols.writing_handler.get()
    }

    /// Sets up the handshake handler, as part of the `Handshaking` protocol.
    pub fn set_handshake_handler(&self, handler: HandshakeHandler) {
        if self.protocols.handshake_handler.set(handler).is_err() {
            panic!("the handshake_handler field was set more than once!");
        }
    }

    /// Sets up the reading handler, as part of enabling the `Reading` protocol.
    pub fn set_reading_handler(&self, handler: ReadingHandler) {
        if self.protocols.reading_handler.set(handler).is_err() {
            panic!("the reading_handler field was set more than once!");
        }
    }

    /// Sets up the writing handler, as part of enabling the `Writing` protocol.
    pub fn set_writing_handler(&self, handler: WritingHandler) {
        if self.protocols.writing_handler.set(handler).is_err() {
            panic!("the writing_handler field was set more than once!");
        }
    }
}

// FIXME: this can probably be done more elegantly
/// Creates the node's tracing span based on its name.
fn create_span(node_name: &str) -> Span {
    let span = trace_span!("node", name = node_name);
    let span = if span.is_disabled() {
        debug_span!("node", name = node_name)
    } else {
        span
    };
    let span = if span.is_disabled() {
        info_span!("node", name = node_name)
    } else {
        span
    };
    let span = if span.is_disabled() {
        warn_span!("node", name = node_name)
    } else {
        span
    };
    let span = if span.is_disabled() {
        error_span!("node", name = node_name)
    } else {
        span
    };
    span
}
