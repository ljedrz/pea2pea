//! A minimal Kademlia DHT - the canonical P2P distributed hash table.
//!
//! Demonstrates:
//! - [`Handshake`]: exchange 64-bit node IDs over the raw TCP stream
//! - [`Reading`] + [`Writing`]: `postcard`-serialized RPC messages over `LengthDelimitedCodec`
//! - [`OnConnect`] / [`OnDisconnect`]: routing-table maintenance as peers come and go
//! - [`Writing::unicast`]: request/response RPCs with correlation IDs (cf. `simple_rpc`)
//! - The iterative lookup: discover peers by XOR-closeness without knowing everyone
//!
//! A real Kademlia uses PING for k-bucket freshness; this demo relies on
//! [`OnDisconnect`] instead, which is simpler and sufficient for a trusted
//! network. See the Kademlia paper (Maymounkov & Mazieres, 2002) for details.

use std::{
    collections::{HashMap, HashSet},
    io,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::Duration,
};

use bytes::BytesMut;
use parking_lot::Mutex;
use pea2pea::{
    Config, Connection, ConnectionSide, DisconnectOrigin, Node, Pea2Pea,
    protocols::{Handshake, OnConnect, OnDisconnect, Reading, Writing},
};
use rand::RngExt;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::oneshot,
    time::timeout,
};
use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec};
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

/// A Kademlia node identifier - a 64-bit key in the XOR distance space.
type NodeId = u64;

/// The replication factor (`k`): max contacts stored and returned per lookup.
const K: usize = 4;

/// A peer contact: its node ID and network address.
#[derive(Clone, Debug)]
struct Contact {
    id: NodeId,
    addr: SocketAddr,
}

// ── Wire protocol ──────────────────────────────────────────────────────

/// All Kademlia RPC messages. Requests carry an `rpc_id` for correlation;
/// responses echo it back so the caller can match them via a oneshot channel.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
enum KademliaMessage {
    /// FIND_NODE - "who are the k closest contacts to `target`?"
    FindNode {
        rpc_id: u64,
        sender_id: NodeId,
        target: NodeId,
    },
    /// FIND_NODE response - a list of close contacts.
    FindNodeResult {
        rpc_id: u64,
        contacts: Vec<(NodeId, String)>,
    },
    /// STORE - "please hold this key/value pair."
    Store {
        rpc_id: u64,
        key: NodeId,
        value: Vec<u8>,
    },
    /// STORE response - "done."
    StoreResult { rpc_id: u64 },
    /// FIND_VALUE - "do you have the value for `key`?"
    FindValue { rpc_id: u64, key: NodeId },
    /// FIND_VALUE response - either the value or a list of close contacts.
    FindValueResult {
        rpc_id: u64,
        result: FindValueOutcome,
    },
}

impl KademliaMessage {
    /// Extract the correlation ID from any message variant.
    fn rpc_id(&self) -> u64 {
        match self {
            Self::FindNode { rpc_id, .. }
            | Self::FindNodeResult { rpc_id, .. }
            | Self::Store { rpc_id, .. }
            | Self::StoreResult { rpc_id }
            | Self::FindValue { rpc_id, .. }
            | Self::FindValueResult { rpc_id, .. } => *rpc_id,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
enum FindValueOutcome {
    /// The value was found.
    Found(Vec<u8>),
    /// The value was not found; here are closer contacts.
    Contacts(Vec<(NodeId, String)>),
}

/// A request to be sent via [`DhtNode::rpc`]; the `rpc_id` is assigned by `rpc`.
enum KademliaRequest {
    FindNode { sender_id: NodeId, target: NodeId },
    Store { key: NodeId, value: Vec<u8> },
    FindValue { key: NodeId },
}

impl KademliaRequest {
    fn into_message(self, rpc_id: u64) -> KademliaMessage {
        match self {
            Self::FindNode { sender_id, target } => KademliaMessage::FindNode {
                rpc_id,
                sender_id,
                target,
            },
            Self::Store { key, value } => KademliaMessage::Store { rpc_id, key, value },
            Self::FindValue { key } => KademliaMessage::FindValue { rpc_id, key },
        }
    }
}

struct KademliaCodec(LengthDelimitedCodec);

impl Default for KademliaCodec {
    fn default() -> Self {
        let inner = LengthDelimitedCodec::builder()
            .length_field_length(2)
            .new_codec();
        Self(inner)
    }
}

impl Decoder for KademliaCodec {
    type Item = KademliaMessage;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.0
            .decode(src)?
            .map(|b| {
                postcard::from_bytes::<Self::Item>(&b)
                    .map_err(|_| io::ErrorKind::InvalidData.into())
            })
            .transpose()
    }
}

impl Encoder<KademliaMessage> for KademliaCodec {
    type Error = io::Error;

    fn encode(&mut self, item: KademliaMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let bytes = postcard::to_stdvec(&item).map_err(|_| io::ErrorKind::InvalidData)?;
        self.0.encode(bytes.into(), dst)
    }
}

// ── The DHT node ───────────────────────────────────────────────────────

#[derive(Clone)]
struct DhtNode {
    node: Node,
    id: NodeId,
    /// Peer info learned during the handshake, keyed by the *connection* address
    /// (i.e. `conn.addr()`). The value is `(peer NodeId, peer listening address)`.
    /// The listening address is what other nodes need to connect to this peer.
    peer_info: Arc<Mutex<HashMap<SocketAddr, (NodeId, SocketAddr)>>>,
    /// The routing table - a flat list of known contacts.
    ///
    /// A production Kademlia uses k-buckets for O(log n) selection; with few
    /// nodes a flat sort-by-XOR-distance is simpler and equally illustrative.
    routing_table: Arc<Mutex<Vec<Contact>>>,
    /// Local key/value store for STORE/FIND_VALUE.
    storage: Arc<Mutex<HashMap<NodeId, Vec<u8>>>>,
    next_rpc_id: Arc<AtomicU64>,
    pending_rpcs: Arc<Mutex<HashMap<u64, oneshot::Sender<KademliaMessage>>>>,
}

impl DhtNode {
    fn new(id: NodeId) -> Self {
        let config = Config {
            name: Some(format!("{id:016x}")),
            listener_addr: Some("127.0.0.1:0".parse().unwrap()),
            max_connections_per_ip: 100,
            ..Default::default()
        };
        Self {
            node: Node::new(config),
            id,
            peer_info: Default::default(),
            routing_table: Default::default(),
            storage: Default::default(),
            next_rpc_id: Default::default(),
            pending_rpcs: Default::default(),
        }
    }

    /// XOR distance between two node IDs - the Kademlia distance metric.
    fn distance(a: NodeId, b: NodeId) -> NodeId {
        a ^ b
    }

    /// The `k` contacts closest to `target` by XOR distance.
    fn closest_contacts(&self, target: NodeId, k: usize) -> Vec<Contact> {
        let mut contacts = self.routing_table.lock().clone();
        contacts.sort_by_key(|c| Self::distance(c.id, target));
        contacts.into_iter().take(k).collect()
    }

    /// Insert a contact into the routing table if not already present.
    fn add_contact(&self, id: NodeId, addr: SocketAddr) {
        if id == self.id {
            return;
        }
        let mut table = self.routing_table.lock();
        if !table.iter().any(|c| c.id == id) {
            debug!(parent: self.node().span(), "routing table: +{id:016x} @ {addr}");
            table.push(Contact { id, addr });
        }
    }

    /// Ensure a connection exists to `addr`, connecting if necessary.
    /// After `connect` returns, the handshake has completed and the peer's
    /// NodeId is available, so the contact can be added to the routing table.
    ///
    /// note: A production DHT dedups by `NodeId`, not `SocketAddr`.
    async fn ensure_connected(&self, addr: SocketAddr) -> bool {
        if self.node().is_connected(addr) {
            return true;
        }
        if self.node().connect(addr).await.is_err() {
            return false;
        }
        if let Some((peer_id, peer_listening_addr)) = self.peer_info.lock().get(&addr).copied() {
            self.add_contact(peer_id, peer_listening_addr);
        }
        true
    }

    /// Send an RPC to a connected peer and await its response (2s timeout).
    async fn rpc(&self, addr: SocketAddr, request: KademliaRequest) -> io::Result<KademliaMessage> {
        let rpc_id = self.next_rpc_id.fetch_add(1, Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending_rpcs.lock().insert(rpc_id, tx);

        let msg = request.into_message(rpc_id);
        if let Err(e) = self.unicast_fast(addr, msg) {
            self.pending_rpcs.lock().remove(&rpc_id);
            return Err(e);
        }

        let res = timeout(Duration::from_secs(2), rx).await;
        self.pending_rpcs.lock().remove(&rpc_id); // always reclaim, timeout or not
        res.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "RPC timed out"))?
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "RPC peer dropped"))
    }

    /// Merge discovered contacts into the routing table and shortlist.
    fn absorb_contacts(&self, contacts: Vec<(NodeId, String)>, shortlist: &mut Vec<Contact>) {
        for (id, addr_str) in contacts {
            if id == self.id {
                continue;
            }
            if let Ok(addr) = addr_str.parse() {
                self.add_contact(id, addr);
                if !shortlist.iter().any(|c| c.id == id) {
                    shortlist.push(Contact { id, addr });
                }
            }
        }
    }

    /// Iterative FIND_NODE: discover the `k` closest contacts to `target`,
    /// connecting to newly discovered peers along the way.
    ///
    /// Sequential (real Kademlia sends α = 3 queries in parallel for latency,
    /// but the sequential version is easier to follow and equally correct).
    async fn iterative_find_node(&self, target: NodeId) -> Vec<Contact> {
        let mut shortlist = self.closest_contacts(target, K);
        let mut queried = HashSet::new();

        loop {
            shortlist.sort_by_key(|c| Self::distance(c.id, target));

            // Pick the closest un-queried contact.
            let contact = match shortlist.iter().find(|c| !queried.contains(&c.id)) {
                Some(c) => c.clone(),
                None => break, // all queried
            };
            queried.insert(contact.id);

            if !self.ensure_connected(contact.addr).await {
                continue;
            }

            let result = self
                .rpc(
                    contact.addr,
                    KademliaRequest::FindNode {
                        sender_id: self.id,
                        target,
                    },
                )
                .await;

            if let Ok(KademliaMessage::FindNodeResult { contacts, .. }) = result {
                self.absorb_contacts(contacts, &mut shortlist);
            }
        }

        shortlist.sort_by_key(|c| Self::distance(c.id, target));
        shortlist.truncate(K);
        shortlist
    }

    /// STORE a key/value pair at the `k` nodes closest to `key`.
    async fn store(&self, key: NodeId, value: &[u8]) {
        let contacts = self.iterative_find_node(key).await;
        for contact in &contacts {
            if !self.ensure_connected(contact.addr).await {
                continue;
            }
            let _ = self
                .rpc(
                    contact.addr,
                    KademliaRequest::Store {
                        key,
                        value: value.to_vec(),
                    },
                )
                .await;
        }
    }

    /// FIND_VALUE: iteratively look up `key` until a node returns the value
    /// or the search exhausts without finding it.
    async fn find_value(&self, key: NodeId) -> Option<Vec<u8>> {
        let mut shortlist = self.closest_contacts(key, K);
        let mut queried = HashSet::new();

        loop {
            shortlist.sort_by_key(|c| Self::distance(c.id, key));

            let contact = match shortlist.iter().find(|c| !queried.contains(&c.id)) {
                Some(c) => c.clone(),
                None => break,
            };
            queried.insert(contact.id);

            if !self.ensure_connected(contact.addr).await {
                continue;
            }

            let result = self
                .rpc(contact.addr, KademliaRequest::FindValue { key })
                .await;

            if let Ok(KademliaMessage::FindValueResult { result, .. }) = result {
                match result {
                    FindValueOutcome::Found(value) => return Some(value),
                    FindValueOutcome::Contacts(contacts) => {
                        self.absorb_contacts(contacts, &mut shortlist);
                    }
                }
            }
        }

        None
    }
}

// ── Pea2Pea + protocol implementations ─────────────────────────────────

impl Pea2Pea for DhtNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Handshake for DhtNode {
    async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
        let side = conn.side();
        let stream = self.borrow_stream(&mut conn);

        // Exchange: 8-byte node ID + 2-byte listening port (big-endian).
        // The listening port lets the peer construct our *connectable* address
        // (conn.addr() for an inbound connection is our ephemeral source port,
        // not our listening port - see the note on peer_info above).
        let my_port = self
            .node()
            .listening_addr()
            .await
            .map(|a| a.port())
            .unwrap_or(0);

        let mut out = [0u8; 10];
        out[..8].copy_from_slice(&self.id.to_be_bytes());
        out[8..].copy_from_slice(&my_port.to_be_bytes());

        let mut buf = [0u8; 10];

        // The `!side` inversion ensures complementary send/receive ordering,
        // avoiding a potential deadlock on both-sides-send-first.
        match !side {
            ConnectionSide::Initiator => {
                stream.write_all(&out).await?;
                stream.read_exact(&mut buf).await?;
            }
            ConnectionSide::Responder => {
                stream.read_exact(&mut buf).await?;
                stream.write_all(&out).await?;
            }
        }

        let peer_id = NodeId::from_be_bytes(buf[..8].try_into().unwrap());
        let peer_port = u16::from_be_bytes(buf[8..].try_into().unwrap());
        // Construct the peer's listening address from the connection's remote IP
        // and the port received in the handshake.
        let peer_listening_addr = SocketAddr::new(conn.addr().ip(), peer_port);

        self.peer_info
            .lock()
            .insert(conn.addr(), (peer_id, peer_listening_addr));

        debug!(
            parent: self.node().span(),
            "handshake: peer {peer_id:016x} listening @ {peer_listening_addr}"
        );

        Ok(conn)
    }
}

impl OnConnect for DhtNode {
    // Non-abortable: routing-table bookkeeping must pair with OnDisconnect.
    const ABORTABLE: bool = false;

    async fn on_connect(&self, addr: SocketAddr) {
        if let Some((peer_id, peer_listening_addr)) = self.peer_info.lock().get(&addr).copied() {
            self.add_contact(peer_id, peer_listening_addr);
        }
    }
}

impl OnDisconnect for DhtNode {
    async fn on_disconnect(&self, addr: SocketAddr, _origin: DisconnectOrigin) {
        if let Some((peer_id, _)) = self.peer_info.lock().remove(&addr) {
            self.routing_table.lock().retain(|c| c.id != peer_id);
            debug!(
                parent: self.node().span(),
                "routing table: -{peer_id:016x} @ {addr}"
            );
        }
    }
}

impl Reading for DhtNode {
    type Message = KademliaMessage;
    type Codec = KademliaCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) {
        match message {
            // ── FIND_NODE ──
            KademliaMessage::FindNode {
                rpc_id,
                sender_id,
                target,
                ..
            } => {
                // Learn about the sender; use its listening address (from the
                // handshake) rather than the connection source address.
                let sender_addr = self
                    .peer_info
                    .lock()
                    .get(&source)
                    .map(|(_, addr)| *addr)
                    .unwrap_or(source);
                self.add_contact(sender_id, sender_addr);

                let contacts: Vec<(NodeId, String)> = self
                    .closest_contacts(target, K)
                    .into_iter()
                    .map(|c| (c.id, c.addr.to_string()))
                    .collect();

                let _ =
                    self.unicast_fast(source, KademliaMessage::FindNodeResult { rpc_id, contacts });
            }

            // ── STORE ──
            KademliaMessage::Store { rpc_id, key, value } => {
                self.storage.lock().insert(key, value);
                let _ = self.unicast_fast(source, KademliaMessage::StoreResult { rpc_id });
            }

            // ── FIND_VALUE ──
            KademliaMessage::FindValue { rpc_id, key } => {
                let result = match self.storage.lock().get(&key) {
                    Some(v) => FindValueOutcome::Found(v.clone()),
                    None => {
                        let contacts: Vec<(NodeId, String)> = self
                            .closest_contacts(key, K)
                            .into_iter()
                            .map(|c| (c.id, c.addr.to_string()))
                            .collect();
                        FindValueOutcome::Contacts(contacts)
                    }
                };

                let _ =
                    self.unicast_fast(source, KademliaMessage::FindValueResult { rpc_id, result });
            }

            // ── Responses: route to the waiting RPC caller ──
            response => {
                let rpc_id = response.rpc_id();
                if let Some(tx) = self.pending_rpcs.lock().remove(&rpc_id) {
                    let _ = tx.send(response);
                }
            }
        }
    }
}

impl Writing for DhtNode {
    type Message = KademliaMessage;
    type Codec = KademliaCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

// ── Demo ───────────────────────────────────────────────────────────────

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    examples::start_logger(LevelFilter::INFO);

    const NUM_NODES: usize = 16;

    // Create N nodes with random 64-bit IDs.
    let mut nodes = Vec::with_capacity(NUM_NODES);
    for _ in 0..NUM_NODES {
        let id: u64 = rand::rng().random();
        let node = DhtNode::new(id);
        node.enable_handshake().await;
        node.enable_reading().await;
        node.enable_writing().await;
        node.enable_on_connect().await;
        node.enable_on_disconnect().await;
        node.node().toggle_listener().await.unwrap();
        nodes.push(node);
    }

    println!("\n── {NUM_NODES} DHT nodes created ──");
    for (i, node) in nodes.iter().enumerate() {
        println!("  node {i:>2}: {:016x}", node.id);
    }

    // Bootstrap: every node connects to node 0.
    let bootstrap_addr = nodes[0].node().listening_addr().await.unwrap();
    for node in nodes.iter().skip(1) {
        node.node().connect(bootstrap_addr).await.unwrap();
    }
    info!("bootstrap complete - every node connected to node 0");

    // Each node discovers closer peers via iterative FIND_NODE(self).
    // Sequential so earlier nodes enrich the network for later ones.
    for (i, node) in nodes.iter().enumerate().skip(1) {
        let found = node.iterative_find_node(node.id).await;
        debug!(
            parent: node.node().span(),
            "node {i} discovery complete: {} contacts",
            found.len()
        );
    }

    // Print routing tables.
    println!("\n── routing tables after discovery ──");
    for (i, node) in nodes.iter().enumerate() {
        let table = node.routing_table.lock();
        let contacts: Vec<String> = table.iter().map(|c| format!("{:016x}", c.id)).collect();
        println!(
            "  node {i:>2} ({:016x}): {} contacts: {}",
            node.id,
            table.len(),
            contacts.join(", ")
        );
    }

    // ── STORE + FIND_VALUE demo ──
    let key: NodeId = 42;
    let value = b"hello from the DHT!".to_vec();

    let storer = &nodes[5];
    info!(
        parent: storer.node().span(),
        "STORE key={key} value={:?}", String::from_utf8_lossy(&value)
    );
    storer.store(key, &value).await;

    // Let stores propagate.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Print storage distribution.
    println!("\n── storage distribution (key={key}) ──");
    for (i, node) in nodes.iter().enumerate() {
        if let Some(v) = node.storage.lock().get(&key) {
            println!(
                "  node {i:>2} ({:016x}): holds key={key}, value={:?}",
                node.id,
                String::from_utf8_lossy(v)
            );
        }
    }

    // A different node looks up the value.
    let finder = &nodes[12];
    info!(parent: finder.node().span(), "FIND_VALUE key={key}");
    match finder.find_value(key).await {
        Some(found) => {
            info!(
                parent: finder.node().span(),
                "FIND_VALUE success! value={:?}", String::from_utf8_lossy(&found)
            );
            println!(
                "\nnode 12 found the value stored by node 5: {:?}",
                String::from_utf8_lossy(&found)
            );
        }
        None => {
            warn!(parent: finder.node().span(), "FIND_VALUE: key not found");
            println!("\nnode 12 could not find the value");
        }
    }

    // Cleanup.
    println!("\n── shutting down ──");
    for node in &nodes {
        node.node().shut_down().await;
    }
}
