use crate::config::*;
use crate::connection::{Connection, ConnectionReader};
use crate::connections::Connections;
use crate::known_peers::KnownPeers;

use once_cell::sync::OnceCell;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc::Sender,
};
use tracing::*;

use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

static SEQUENTIAL_NODE_ID: AtomicUsize = AtomicUsize::new(0);

pub struct Node {
    span: Span,
    pub config: NodeConfig,
    pub local_addr: SocketAddr,
    pub incoming_requests: OnceCell<Option<Sender<(Vec<u8>, SocketAddr)>>>,
    connections: Connections,
    known_peers: KnownPeers,
}

impl Node {
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

        let span = trace_span!("node", name = config.name.as_deref().unwrap());

        let desired_listener = if let Some(port) = config.desired_listening_port {
            let desired_local_addr = SocketAddr::new(local_ip, port);
            TcpListener::bind(desired_local_addr).await
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
        let local_addr = listener.local_addr()?;

        let node = Arc::new(Self {
            span,
            config,
            local_addr,
            incoming_requests: Default::default(),
            connections: Default::default(),
            known_peers: Default::default(),
        });

        let node_clone = Arc::clone(&node);
        tokio::spawn(async move {
            debug!(parent: node_clone.span(), "spawned a listening task");
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => Arc::clone(&node_clone).accept_connection(stream, addr),
                    Err(e) => {
                        error!(parent: node_clone.span(), "couldn't accept a connection: {}", e);
                    }
                }
            }
        });

        info!(
            parent: node.span(),
            "the node is ready; listening on {}",
            local_addr
        );

        Ok(node)
    }

    pub fn name(&self) -> &str {
        // safe; can be set as None in NodeConfig, but receives a default value on Node creation
        self.config.name.as_deref().unwrap()
    }

    pub fn span(&self) -> Span {
        self.span.clone()
    }

    fn adapt_stream(self: &Arc<Self>, stream: TcpStream, addr: SocketAddr) {
        debug!(parent: self.span(), "connecting to {}", addr);
        let (reader, writer) = stream.into_split();

        let mut connection_reader = ConnectionReader::new(reader, Arc::clone(&self));

        let node = Arc::clone(&self);
        let reader_task = tokio::spawn(async move {
            debug!(parent: node.span(), "spawned a task reading messages from {}", addr);
            loop {
                match connection_reader.read_message().await {
                    Ok(msg) => {
                        info!(parent: node.span(), "received a {}B message from {}", msg.len(), addr);

                        node.known_peers.register_incoming_message(addr, msg.len());

                        if let Some(Some(ref incoming_requests)) = node.incoming_requests.get() {
                            if let Err(e) = incoming_requests.send((msg, addr)).await {
                                error!(parent: node.span(), "can't register an incoming message: {}", e);
                                // TODO: how to proceed?
                            }
                        }
                    }
                    Err(e) => {
                        node.known_peers.register_failure(addr);
                        error!(parent: node.span(), "can't read message: {}", e);
                    }
                }
            }
        });

        let connection = Connection::new(reader_task, writer, Arc::clone(&self));
        self.connections
            .handshaking
            .write()
            .insert(addr, connection);
    }

    fn accept_connection(self: Arc<Self>, stream: TcpStream, addr: SocketAddr) {
        self.known_peers.add(addr);
        self.adapt_stream(stream, addr);
    }

    pub async fn initiate_connection(self: &Arc<Self>, addr: SocketAddr) -> io::Result<()> {
        if self.connections.is_connected(addr) {
            warn!(parent: self.span(), "already connecting/connected to {}", addr);
            return Ok(());
        }

        self.known_peers.add(addr);
        let stream = TcpStream::connect(addr).await?;
        self.adapt_stream(stream, addr);

        Ok(())
    }

    pub fn disconnect(&self, addr: SocketAddr) -> bool {
        let disconnected = self.connections.disconnect(addr);

        if disconnected {
            info!(parent: self.span(), "disconnected from {}", addr);
        } else {
            warn!(parent: self.span(), "wasn't connected to {}", addr);
        }

        disconnected
    }

    pub async fn send_direct_message(&self, addr: SocketAddr, message: Vec<u8>) -> io::Result<()> {
        let ret = self.connections.send_direct_message(addr, message).await;

        if let Err(ref e) = ret {
            error!(parent: self.span(), "couldn't send a direct message to {}: {}", addr, e);
        }

        ret
    }

    pub async fn send_broadcast(&self, message: Vec<u8>) {
        for (addr, conn) in self.connections.handshaken_connections().iter() {
            // FIXME: it would be nice not to clone the message
            if let Err(e) = conn.lock().await.send_message(message.clone()).await {
                error!(parent: self.span(), "couldn't send a broadcast to {}: {}", addr, e);
            }
        }
    }

    pub fn is_connected(&self, addr: SocketAddr) -> bool {
        self.connections.is_connected(addr)
    }

    pub fn num_connected(&self) -> usize {
        self.connections.num_connected()
    }

    pub fn is_handshaking(&self, addr: SocketAddr) -> bool {
        self.connections.is_handshaking(addr)
    }

    pub fn is_handshaken(&self, addr: SocketAddr) -> bool {
        self.connections.is_handshaken(addr)
    }

    pub fn num_messages_received(&self) -> usize {
        self.known_peers.num_messages_received()
    }

    // to be used in tests only, at least until the handshake protocol is introduced (which should make it redundant)
    #[doc(hidden)]
    pub fn mark_as_handshaken(&self, addr: SocketAddr) {
        if let Err(e) = self.connections.mark_as_handshaken(addr) {
            error!(parent: self.span(), "can't mark {} as handshaken: {}", addr, e);
        }
    }
}
