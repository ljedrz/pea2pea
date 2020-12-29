use crate::config::NodeConfig;
use crate::connection::{Connection, ConnectionReader, ConnectionSide};
use crate::connections::Connections;
use crate::known_peers::KnownPeers;
use crate::protocols::{HandshakeSetup, InboundMessages, Protocols, ReadingClosure};

use bytes::Bytes;
use tokio::net::{TcpListener, TcpStream};
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

/// A trait for objects containing a `Node`; it is required to implement protocols.
pub trait ContainsNode {
    /// Returns a clonable reference to the node.
    fn node(&self) -> &Arc<Node>;
}

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

        let desired_listener = if let Some(port) = config.desired_listening_port {
            let desired_listening_addr = SocketAddr::new(local_ip, port);
            TcpListener::bind(desired_listening_addr).await
        } else if config.allow_random_port {
            let random_available_addr = SocketAddr::new(local_ip, 0);
            TcpListener::bind(random_available_addr).await
        } else {
            panic!("you must either provide a desired port or allow a random port to be chosen");
        };

        let listener = match desired_listener {
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
        };
        let listening_addr = listener.local_addr()?;

        let node = Arc::new(Self {
            span,
            config,
            listening_addr,
            protocols: Default::default(),
            connections: Default::default(),
            known_peers: Default::default(),
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

    /// Returns a vector of `Node`s wrapped in `Arc`s.
    pub async fn new_multiple(
        count: usize,
        config: Option<NodeConfig>,
    ) -> io::Result<Vec<Arc<Self>>> {
        let mut nodes = Vec::with_capacity(count);

        for _ in 0..count {
            let node = Node::new(config.clone()).await?;
            nodes.push(node);
        }

        Ok(nodes)
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

        let connection_reader = ConnectionReader::new(peer_addr, reader, Arc::clone(&self));
        let connection = Connection::new(peer_addr, writer, Arc::clone(&self), !own_side);

        let (connection_reader, connection) = if let Some(ref handshake_setup) =
            self.handshake_setup()
        {
            let handshake_task = match own_side {
                ConnectionSide::Initiator => {
                    (handshake_setup.initiator_closure)(connection_reader, connection)
                }
                ConnectionSide::Responder => {
                    (handshake_setup.responder_closure)(connection_reader, connection)
                }
            };

            match handshake_task.await {
                Ok(Ok((conn_reader, conn, handshake_state))) => {
                    if let Some(ref sender) = handshake_setup.state_sender {
                        // can't recover from an error here
                        sender
                            .send((peer_addr, handshake_state))
                            .await
                            .expect("the handshake state channel is closed")
                    }
                    debug!(parent: self.span(), "marked {} as handshaken", peer_addr);

                    (conn_reader, conn)
                }
                _ => {
                    error!(parent: self.span(), "handshake with {} failed; dropping the connection", peer_addr);
                    self.known_peers().register_failure(peer_addr);
                    return Err(ErrorKind::Other.into());
                }
            }
        } else {
            (connection_reader, connection)
        };

        if let Some(ref messaging_closure) = self.reading_closure() {
            connection
                .reader_task
                .set(messaging_closure(connection_reader))
                .unwrap();
        }

        self.connections.add(connection);
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

    /// Sends the provided message to the specified `SocketAddr`.
    pub async fn send_direct_message(&self, addr: SocketAddr, message: Bytes) -> io::Result<()> {
        if let Some(message_sender) = self.connections.sender(addr) {
            message_sender
                .send(message)
                .await
                .expect("the connection writer task is closed"); // can't recover if this happens
            Ok(())
        } else {
            Err(ErrorKind::NotConnected.into())
        }
    }

    /// Broadcasts the provided message to all peers.
    pub async fn send_broadcast(&self, message: Bytes) {
        for message_sender in self.connections.senders() {
            message_sender
                .send(message.clone())
                .await
                .expect("the connection writer task is closed"); // can't recover if this happens
        }
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

    /// Returns the number of all sent messages.
    pub fn num_messages_sent(&self) -> usize {
        self.known_peers.num_messages_sent()
    }

    /// Returns the number of all received messages.
    pub fn num_messages_received(&self) -> usize {
        self.known_peers.num_messages_received()
    }

    /// Returns a `Sender` for the channel handling all the
    /// incoming messages, if Messaging is enabled.
    pub fn inbound_messages(&self) -> Option<&InboundMessages> {
        self.protocols.inbound_messages.get()
    }

    /// Returns the closure responsible for spawning per-connection
    /// tasks for handling inbound messages, if Messaging is enabled.
    fn reading_closure(&self) -> Option<&ReadingClosure> {
        self.protocols.reading_closure.get()
    }

    /// Returns a handle to the handshake-relevant objects, if Handshaking is enabled.
    fn handshake_setup(&self) -> Option<&HandshakeSetup> {
        self.protocols.handshake_setup.get()
    }

    /// Sets up the `Sender` for handling all incoming messages, as part of the Messaging protocol.
    pub fn set_inbound_messages(&self, sender: InboundMessages) {
        self.protocols
            .inbound_messages
            .set(sender)
            .expect("the inbound_messages field was set more than once!");
    }

    /// Sets up the closure responsible for spawning per-connection
    /// tasks for handling inbound messages, as part of the Messaging protocol.
    pub fn set_reading_closure(&self, closure: ReadingClosure) {
        if self.protocols.reading_closure.set(closure).is_err() {
            panic!("the reading_closure field was set more than once!");
        }
    }

    /// Sets up the handshake-relevant objects, as part of the Handshaking protocol.
    pub fn set_handshake_setup(&self, closures: HandshakeSetup) {
        if self.protocols.handshake_setup.set(closures).is_err() {
            panic!("the handshake_setup field was set more than once!");
        }
    }
}

impl ContainsNode for Arc<Node> {
    fn node(&self) -> &Arc<Node> {
        &self
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
