use crate::config::*;
use crate::connection::{Connection, ConnectionReader, ConnectionSide};
use crate::connections::Connections;
use crate::known_peers::KnownPeers;
use crate::protocols::{HandshakeClosures, ReadingClosure, WritingClosure};

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

type IncomingRequests = Sender<(Vec<u8>, SocketAddr)>;

pub trait ContainsNode {
    fn node(&self) -> &Node;
}

pub struct Node {
    span: Span,
    pub config: NodeConfig,
    pub local_addr: SocketAddr,
    reading_closure: OnceCell<Option<ReadingClosure>>,
    writing_closure: OnceCell<Option<WritingClosure>>,
    incoming_requests: OnceCell<Option<IncomingRequests>>,
    handshake_closures: OnceCell<Option<HandshakeClosures>>,
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
            reading_closure: Default::default(),
            writing_closure: Default::default(),
            handshake_closures: Default::default(),
            connections: Default::default(),
            known_peers: Default::default(),
        });

        let node_clone = Arc::clone(&node);
        tokio::spawn(async move {
            debug!(parent: node_clone.span(), "spawned a listening task");
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        Arc::clone(&node_clone)
                            .accept_connection(stream, addr)
                            .await
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

    async fn adapt_stream(
        self: &Arc<Self>,
        stream: TcpStream,
        addr: SocketAddr,
        side: ConnectionSide,
    ) {
        debug!(parent: self.span(), "connecting to {}", addr);
        let (reader, writer) = stream.into_split();

        let connection_reader = ConnectionReader::new(reader, Arc::clone(&self));
        let connection = Arc::new(Connection::new(writer, Arc::clone(&self)));

        self.connections
            .handshaking
            .write()
            .insert(addr, Arc::clone(&connection));

        let node = Arc::clone(&self);
        let connection_reader =
            if let Some(Some(ref handshake_closures)) = self.handshake_closures.get() {
                let handshake_task = match side {
                    ConnectionSide::Initiator => {
                        (handshake_closures.initiator)(addr, connection_reader, connection)
                    }
                    ConnectionSide::Responder => {
                        (handshake_closures.responder)(addr, connection_reader, connection)
                    }
                };

                match handshake_task.await {
                    Ok(conn_reader) => conn_reader,
                    Err(e) => {
                        error!(parent: self.span(), "handshake with {} failed: {}", addr, e);
                        return;
                        // TODO: cleanup, probably return some Result instead
                    }
                }
            } else {
                connection_reader
            };

        let node = Arc::clone(&self);
        let reader_task = if let Some(Some(ref reading_closure)) = node.reading_closure.get() {
            Some(reading_closure(connection_reader, addr))
        } else {
            None
        };

        if let Err(e) = self.connections.mark_as_handshaken(addr, reader_task).await {
            error!(parent: self.span(), "can't mark {} as handshaken: {}", addr, e);
        } else {
            debug!(parent: self.span(), "marked {} as handshaken", addr);
        }
    }

    async fn accept_connection(self: Arc<Self>, stream: TcpStream, addr: SocketAddr) {
        self.known_peers.add(addr);
        self.adapt_stream(stream, addr, ConnectionSide::Responder)
            .await;
    }

    pub async fn initiate_connection(self: &Arc<Self>, addr: SocketAddr) -> io::Result<()> {
        if self.connections.is_connected(addr) {
            warn!(parent: self.span(), "already connecting/connected to {}", addr);
            return Ok(());
        }

        self.known_peers.add(addr);
        let stream = TcpStream::connect(addr).await?;
        self.adapt_stream(stream, addr, ConnectionSide::Initiator)
            .await;

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
            if let Err(e) = conn.send_message(message.clone()).await {
                error!(parent: self.span(), "couldn't send a broadcast to {}: {}", addr, e);
            }
        }
    }

    pub fn register_received_message(&self, from: SocketAddr, len: usize) {
        self.known_peers.register_received_message(from, len)
    }

    pub fn register_failure(&self, from: SocketAddr) {
        self.known_peers.register_failure(from)
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

    pub async fn mark_as_handshaken(&self, addr: SocketAddr) -> io::Result<()> {
        self.connections.mark_as_handshaken(addr, None).await
    }

    pub fn incoming_requests(&self) -> Option<&IncomingRequests> {
        self.incoming_requests
            .get()
            .and_then(|inner| inner.as_ref())
    }

    pub fn reading_closure(&self) -> Option<&ReadingClosure> {
        self.reading_closure.get().and_then(|inner| inner.as_ref())
    }

    pub fn writing_closure(&self) -> Option<&WritingClosure> {
        self.writing_closure.get().and_then(|inner| inner.as_ref())
    }

    pub fn handshake_closures(&self) -> Option<&HandshakeClosures> {
        self.handshake_closures
            .get()
            .and_then(|inner| inner.as_ref())
    }

    pub fn set_incoming_requests(&self, sender: IncomingRequests) {
        self.incoming_requests
            .set(Some(sender))
            .expect("the incoming_requests field was set more than once!");
    }

    pub fn set_reading_closure(&self, closure: ReadingClosure) {
        if self.reading_closure.set(Some(closure)).is_err() {
            panic!("the reading_closure field was set more than once!");
        }
    }

    pub fn set_writing_closure(&self, closure: WritingClosure) {
        if self.writing_closure.set(Some(closure)).is_err() {
            panic!("the writing_closure field was set more than once!");
        }
    }

    pub fn set_handshake_closures(&self, closures: HandshakeClosures) {
        if self.handshake_closures.set(Some(closures)).is_err() {
            panic!("the handshake_closures field was set more than once!");
        }
    }
}
