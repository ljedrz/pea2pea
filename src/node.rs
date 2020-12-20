use crate::config::*;
use crate::connection::{Connection, ConnectionReader};
use crate::peer_stats::PeerStats;

use parking_lot::RwLock;
use tokio::net::{TcpListener, TcpStream};
use tracing::*;

use std::{
    collections::hash_map::{Entry, HashMap},
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};

pub struct Node {
    pub config: NodeConfig,
    pub local_addr: SocketAddr,
    connecting: RwLock<HashMap<SocketAddr, Arc<Mutex<Connection>>>>,
    connected: RwLock<HashMap<SocketAddr, Arc<Mutex<Connection>>>>,
    known_peers: RwLock<HashMap<SocketAddr, PeerStats>>,
}

impl Node {
    pub async fn new(config: Option<NodeConfig>) -> io::Result<Arc<Self>> {
        let local_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let config = config.unwrap_or_default();

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
                    warn!("trying any port, the desired one is unavailable: {}", e);
                    let random_available_addr = SocketAddr::new(local_ip, 0);
                    TcpListener::bind(random_available_addr).await?
                } else {
                    error!("the desired port is unavailable: {}", e);
                    return Err(e);
                }
            }
        };
        let local_addr = listener.local_addr()?;

        let node = Arc::new(Self {
            config,
            local_addr,
            connecting: Default::default(),
            connected: Default::default(),
            known_peers: Default::default(),
        });

        let node_clone = Arc::clone(&node);
        tokio::spawn(async move {
            debug!("spawned a listening task");
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => Arc::clone(&node_clone).accept_connection(stream, addr),
                    Err(e) => error!("couldn't accept a connection: {}", e),
                }
            }
        });

        info!("the node is ready; listening on {}", local_addr);

        Ok(node)
    }

    fn adapt_stream(self: &Arc<Self>, stream: TcpStream, addr: SocketAddr) {
        let (reader, writer) = stream.into_split();

        let mut connection_reader = ConnectionReader::new(reader, Arc::clone(&self));

        let node = Arc::clone(&self);
        let reader_task = tokio::spawn(async move {
            debug!("spawned a task reading messages from {}", addr);
            loop {
                match connection_reader.read_message().await {
                    Ok(msg_len) => {
                        info!("received a message of {}B from {}", msg_len, addr);
                        node.known_peers
                            .write()
                            .get_mut(&addr)
                            .unwrap()
                            .got_message(msg_len);
                    }
                    Err(e) => error!("can't read message: {}", e),
                }
            }
        });

        let connection = Connection::new(reader_task, writer, Arc::clone(&self));
        self.connecting.write().insert(addr, connection);
    }

    fn accept_connection(self: Arc<Self>, stream: TcpStream, addr: SocketAddr) {
        match self.known_peers.write().entry(addr) {
            Entry::Vacant(e) => {
                e.insert(Default::default());
            }
            Entry::Occupied(mut e) => {
                e.get_mut().new_connection();
            }
        }

        self.adapt_stream(stream, addr);
    }

    pub async fn initiate_connection(self: &Arc<Self>, addr: SocketAddr) -> io::Result<()> {
        if self.is_connecting(addr) || self.is_connected(addr) {
            warn!("already connecting/connected to {}", addr);
            return Ok(());
        }
        debug!("connecting to {}", addr);

        let stream = TcpStream::connect(addr).await?;
        self.adapt_stream(stream, addr);

        Ok(())
    }

    pub fn disconnect(&self, addr: SocketAddr) -> bool {
        let disconnected = if self.connecting.write().remove(&addr).is_none() {
            self.connected.write().remove(&addr).is_some()
        } else {
            true
        };

        if disconnected {
            debug!("disconnected from {}", addr);
        } else {
            warn!("wasn't connected to {}", addr);
        }

        disconnected
    }

    fn perform_handshake(&self) -> io::Result<()> {
        unimplemented!()
    }

    pub fn is_connecting(&self, addr: SocketAddr) -> bool {
        self.connecting.read().contains_key(&addr)
    }

    pub fn is_connected(&self, addr: SocketAddr) -> bool {
        self.connected.read().contains_key(&addr)
    }

    pub fn num_connecting(&self) -> usize {
        self.connecting.read().len()
    }

    pub fn num_connected(&self) -> usize {
        self.connected.read().len()
    }
}
