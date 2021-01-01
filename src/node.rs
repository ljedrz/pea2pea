use crate::connections::{
    Connection, ConnectionReader, ConnectionSide, ConnectionWriter, Connections,
};
use crate::protocols::{HandshakeHandler, Protocols, ReadingHandler, WritingHandler};
use crate::*;

use bytes::Bytes;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::oneshot,
};
use tracing::*;

use std::{
    io::{self, ErrorKind},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

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
    /// Collects statistics related to the node's connections.
    known_peers: KnownPeers,
    /// Collects statistics related to the node itself.
    pub stats: NodeStats,
}

impl Node {
    /// Returns a `Node` wrapped in an `Arc`.
    pub async fn new(config: Option<NodeConfig>) -> io::Result<Arc<Self>> {
        let local_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let mut config = config.unwrap_or_default();

        if config.name.is_none() {
            config.name = Some(
                SEQUENTIAL_NODE_ID
                    .fetch_add(1, Ordering::SeqCst)
                    .to_string(),
            );
        }

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
        });

        let node_clone = Arc::clone(&node);
        tokio::spawn(async move {
            trace!(parent: node_clone.span(), "spawned a listening task");
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        if let Err(e) = node_clone
                            .adapt_stream(stream, addr, ConnectionSide::Responder)
                            .await
                        {
                            error!(parent: node_clone.span(), "couldn't accept a connection: {}", e);
                        }
                    }
                    Err(e) => {
                        error!(parent: node_clone.span(), "couldn't accept a connection: {}", e);
                    }
                }
            }
        });

        info!(
            parent: node.span(),
            "the node is ready; listening on {}",
            listening_addr
        );

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

    /// Prepares the freshly acquired connection to handle the protocols it implements.
    async fn adapt_stream(
        self: &Arc<Self>,
        stream: TcpStream,
        peer_addr: SocketAddr,
        own_side: ConnectionSide,
    ) -> io::Result<()> {
        self.known_peers.add(peer_addr);

        // register the port seen by the peer
        let port_seen_by_peer = if let ConnectionSide::Responder = own_side {
            peer_addr.port()
        } else if let Ok(addr) = stream.local_addr() {
            addr.port()
        } else {
            error!(parent: self.span(), "couldn't determine the connection responder's port");
            return Err(ErrorKind::Other.into());
        };

        debug!(parent: self.span(), "establishing connection with {}{}", peer_addr,
            if peer_addr.port() != port_seen_by_peer {
                format!("; the peer is connected on port {}", port_seen_by_peer)
            } else {
                "".into()
            }
        );

        let (reader, writer) = stream.into_split();
        let reader = ConnectionReader::new(peer_addr, reader, self);
        let writer = ConnectionWriter::new(peer_addr, writer, self);

        let connection = Connection::new(peer_addr, !own_side, self);

        // Handshaking
        let (reader, writer) = if let Some(ref handshake_handler) = self.handshake_handler() {
            let (handshake_result_sender, handshake_result_receiver) = oneshot::channel();

            handshake_handler
                .send((reader, writer, own_side, handshake_result_sender))
                .await;

            match handshake_result_receiver.await {
                Ok(Ok(reader_and_writer)) => reader_and_writer,
                _ => {
                    error!(parent: self.span(), "handshake with {} failed; dropping the connection", peer_addr);
                    self.known_peers().register_failure(peer_addr);
                    return Err(ErrorKind::Other.into());
                }
            }
        } else {
            (reader, writer)
        };

        // Reading
        let connection = if let Some(ref reading_handler) = self.reading_handler() {
            let (conn_returner, conn_retriever) = oneshot::channel();

            reading_handler
                .send((reader, connection, conn_returner))
                .await;

            match conn_retriever.await {
                Ok(Ok(conn)) => conn,
                _ => {
                    error!(parent: self.span(), "can't spawn reading handlers; dropping the connection with {}", peer_addr);
                    self.known_peers().register_failure(peer_addr);
                    return Err(ErrorKind::Other.into());
                }
            }
        } else {
            connection
        };

        // Writing
        let connection = if let Some(ref writing_handler) = self.writing_handler() {
            let (conn_returner, conn_retriever) = oneshot::channel();

            writing_handler
                .send((writer, connection, conn_returner))
                .await;

            match conn_retriever.await {
                Ok(Ok(conn)) => conn,
                _ => {
                    error!(parent: self.span(), "can't spawn writing handlers; dropping the connection with {}", peer_addr);
                    self.known_peers().register_failure(peer_addr);
                    return Err(ErrorKind::Other.into());
                }
            }
        } else {
            connection
        };

        self.connections.add(connection);
        self.stats.register_connection();
        self.known_peers.register_connection(peer_addr);

        Ok(())
    }

    /// Connects to the provided `SocketAddr`.
    pub async fn initiate_connection(self: &Arc<Self>, addr: SocketAddr) -> io::Result<()> {
        if self.connections.is_connected(addr) {
            warn!(parent: self.span(), "already connecting/connected to {}", addr);
            return Err(ErrorKind::Other.into());
        }

        let stream = TcpStream::connect(addr).await?;
        self.adapt_stream(stream, addr, ConnectionSide::Initiator)
            .await
    }

    /// Disconnects from the provided `SocketAddr`.
    pub fn disconnect(&self, addr: SocketAddr) -> bool {
        let disconnected = self.connections.disconnect(addr);

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
