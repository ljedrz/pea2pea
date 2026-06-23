//! NAT traversal via **true TCP simultaneous open** (a.k.a. TCP hole punching).
//!
//! This is the genuine-hole-punching counterpart to the `nat_traversal`
//! example. Where that one elects a single initiator and has the other peer
//! simply listen (which only works when the listener is already reachable),
//! here **both peers dial each other at once** from a socket pre-bound to the
//! same local port they advertised. Each peer's outbound SYN opens its own
//! NAT mapping; the crossing SYN then finds that mapping in place and the two
//! half-connections collapse into a single established TCP connection
//! (RFC 9293 §3.5). This mutual-outbound step is what actually traverses a
//! NAT - a peer that only listens never punches a hole.
//!
//! The pea2pea primitive that makes this possible is
//! [`Node::connect_using_socket`]: it lets you bring your own pre-bound
//! `TcpSocket` instead of letting the library open an ephemeral one, so you
//! control the source port the SYN carries.
//!
//! ## Why there is no listener
//!
//! A real simultaneous open produces **one** connection per peer, reached over
//! the connecting socket itself - there is no separate inbound accept. So the
//! peers run with `listener_addr: None` and rely solely on their punch sockets.
//! This is deliberate: if a peer also kept a live listener on the punch port,
//! a SYN that arrived a hair before the local `connect()` would be accepted by
//! the listener as an ordinary inbound connection, and the subsequent dial
//! would create a *second* connection - exactly the duplicate-connection
//! problem the `nat_traversal` example tie-breaks around. Dropping the listener
//! removes the race: a mistimed SYN simply gets a RST and the sender retries
//! until the two genuinely cross.
//!
//! ## Flow
//!
//! 1. A public **rendezvous** server reachable by both peers listens on a known
//!    address.
//! 2. Each peer reserves a local punch port and announces it to the rendezvous.
//! 3. Once two peers have registered, the rendezvous tells each about the
//!    other's punch address.
//! 4. Both peers repeatedly dial the other *from their own punch port* until a
//!    pair of SYNs crosses and the simultaneous open succeeds.
//! 5. The single resulting connection is used for P2P messaging over pea2pea.
//!
//! ## Running
//!
//! ```text
//! cargo run --example hole_punching -- rendezvous
//! cargo run --example hole_punching -- alice
//! cargo run --example hole_punching -- bob
//! ```
//!
//! ## What this is *not*
//!
//! - **STUN.** Peers advertise their *local* punch address. On loopback or a
//!   1:1-mapped NAT that is also the externally reachable address; behind a
//!   real NAT you would learn the external `IP:port` via STUN and announce
//!   that instead. Nothing else changes.
//! - **A symmetric-NAT solution.** Symmetric NATs assign an unpredictable
//!   external port per destination, so the advertised mapping won't match the
//!   one the peer's SYN creates. Those paths need a TURN-style relay fallback.
//! - **TCP simultaneous open is best-effort.** Some middleboxes RST an
//!   unexpected inbound SYN; production systems usually prefer UDP hole punching
//!   and keep TCP as a secondary path. pea2pea being TCP-only, this is the
//!   technique available - and it is sufficient to show traversal is viable.

use std::{env, io, net::SocketAddr, time::Duration};

use bytes::{Bytes, BytesMut};
use pea2pea::{
    Config, ConnectionSide, Node, Pea2Pea,
    protocols::{Reading, Writing},
};
use tokio::{net::TcpSocket, time::sleep};
use tokio_util::codec::LengthDelimitedCodec;
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

const DEFAULT_RENDEZVOUS: &str = "127.0.0.1:9000";

/// How many times a peer will retry the simultaneous-open connection before
/// giving up. Each retry is a fresh pair of crossing SYNs.
const HOLE_PUNCH_ATTEMPTS: usize = 20;
/// Delay between simultaneous-open attempts. Kept short so the two peers'
/// dials overlap within roughly one round-trip.
const HOLE_PUNCH_INTERVAL: Duration = Duration::from_millis(100);

/// Wait this long after connecting to the rendezvous before registering;
/// gives the reading/writing tasks a moment to spin up.
const POST_CONNECT_SETTLE: Duration = Duration::from_millis(50);

// === The hole-punching primitive ===

/// Create a `TcpSocket` bound to `bind_addr` with `SO_REUSEADDR`, ready to be
/// handed to [`Node::connect_using_socket`].
///
/// `SO_REUSEADDR` is what lets us rebind the *same* punch port on every retry:
/// a socket from a failed attempt may briefly linger, and without it the next
/// `bind` to that port would fail. (Note this is rebinding a single port across
/// time - it is *not* two live sockets sharing a port, which would require
/// `SO_REUSEPORT`. Because there is no listener here, only one socket ever holds
/// the punch port at a time.)
fn punch_socket(bind_addr: SocketAddr) -> io::Result<TcpSocket> {
    let socket = if bind_addr.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    socket.set_reuseaddr(true)?;
    socket.bind(bind_addr)?;
    Ok(socket)
}

/// Reserve a local punch port. We bind a throwaway socket to an ephemeral port,
/// read the address the OS assigned, then drop the socket. Thanks to
/// `SO_REUSEADDR` we can rebind that exact port for the real punch attempts.
fn reserve_punch_addr() -> io::Result<SocketAddr> {
    let probe = punch_socket((std::net::Ipv4Addr::LOCALHOST, 0).into())?;
    probe.local_addr()
    // `probe` is dropped here, releasing the port for `punch_socket` to rebind
}

// === Signaling protocol (peer <-> rendezvous) ===

fn parse_signal(text: &str) -> Option<(&str, &str, SocketAddr)> {
    let (tag, rest) = text.split_once('|')?;
    let (name, addr_str) = rest.split_once('|')?;
    let addr: SocketAddr = addr_str.parse().ok()?;
    Some((tag, name, addr))
}

fn encode_register(name: &str, punch_addr: SocketAddr) -> Bytes {
    Bytes::from(format!("register|{name}|{punch_addr}"))
}

fn encode_peer(name: &str, addr: SocketAddr) -> Bytes {
    Bytes::from(format!("peer|{name}|{addr}"))
}

// === Rendezvous server ===

struct PendingPeer {
    conn_addr: SocketAddr,
    name: String,
    punch_addr: SocketAddr,
}

#[derive(Clone)]
struct Rendezvous {
    node: Node,
    /// Holds the first peer of a pair until the second one shows up.
    pending: std::sync::Arc<parking_lot::Mutex<Option<PendingPeer>>>,
}

impl Pea2Pea for Rendezvous {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Rendezvous {
    fn new(bind_addr: SocketAddr) -> Self {
        let config = Config {
            name: Some("rendezvous".into()),
            listener_addr: Some(bind_addr),
            // many peers connect from the same loopback IP during testing; the
            // default of 1 would reject everything past the first connection
            max_connections_per_ip: 128,
            ..Default::default()
        };
        Self {
            node: Node::new(config),
            pending: Default::default(),
        }
    }

    fn pair(&self, peer: PendingPeer) {
        let mut guard = self.pending.lock();
        if let Some(other) = guard.take() {
            info!(
                parent: self.node.span(),
                "pairing {} ({}) <-> {} ({})",
                other.name, other.punch_addr, peer.name, peer.punch_addr,
            );
            // each peer gets the other's punch address to dial
            if let Err(e) = self.unicast(other.conn_addr, encode_peer(&peer.name, peer.punch_addr))
            {
                warn!(parent: self.node.span(), "couldn't notify {}: {e}", other.name);
            }
            if let Err(e) = self.unicast(peer.conn_addr, encode_peer(&other.name, other.punch_addr))
            {
                warn!(parent: self.node.span(), "couldn't notify {}: {e}", peer.name);
            }
        } else {
            info!(parent: self.node.span(), "{} is waiting for a partner", peer.name);
            *guard = Some(peer);
        }
    }
}

impl Reading for Rendezvous {
    type Message = BytesMut;
    type Codec = LengthDelimitedCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        LengthDelimitedCodec::new()
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) {
        let text = match std::str::from_utf8(&message) {
            Ok(t) => t,
            Err(_) => {
                warn!(parent: self.node.span(), "ignoring non-UTF8 message from {source}");
                return;
            }
        };
        let Some((tag, name, addr)) = parse_signal(text) else {
            warn!(parent: self.node.span(), "ignoring malformed message from {source}: {text:?}");
            return;
        };
        if tag != "register" {
            warn!(parent: self.node.span(), "unexpected tag from {source}: {tag:?}");
            return;
        }
        self.pair(PendingPeer {
            conn_addr: source,
            name: name.to_string(),
            punch_addr: addr,
        });
    }
}

impl Writing for Rendezvous {
    type Message = Bytes;
    type Codec = LengthDelimitedCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        LengthDelimitedCodec::new()
    }
}

// === Peer node ===

#[derive(Clone)]
struct Peer {
    node: Node,
    name: String,
    /// The local port we punch from. Advertised to the rendezvous and used as
    /// the source for every SYN we send, so the counterparty's SYN can cross
    /// ours. Fixed at construction (before any clone) so the reading-task clone
    /// sees the right value.
    punch_addr: SocketAddr,
    rendezvous_addr: SocketAddr,
}

impl Pea2Pea for Peer {
    fn node(&self) -> &Node {
        &self.node
    }
}

impl Peer {
    fn new(name: String, rendezvous_addr: SocketAddr, punch_addr: SocketAddr) -> Self {
        let config = Config {
            name: Some(name.clone()),
            // No listener: a true simultaneous open is carried entirely by the
            // punch sockets. See the module docs for why a live listener would
            // reintroduce duplicate connections.
            listener_addr: None,
            max_connections_per_ip: 128,
            ..Default::default()
        };
        Self {
            node: Node::new(config),
            name,
            punch_addr,
            rendezvous_addr,
        }
    }

    /// Repeatedly dial `peer_addr` from our punch port until a simultaneous
    /// open succeeds. Both peers run this at once; the connection forms as soon
    /// as a pair of SYNs crosses.
    async fn punch_to(&self, peer_addr: SocketAddr) {
        info!(
            parent: self.node.span(),
            "punching to {peer_addr} from {} (simultaneous open)",
            self.punch_addr,
        );

        for attempt in 1..=HOLE_PUNCH_ATTEMPTS {
            let socket = match punch_socket(self.punch_addr) {
                Ok(s) => s,
                Err(e) => {
                    warn!(parent: self.node.span(), "punch bind failed (attempt {attempt}): {e}");
                    sleep(HOLE_PUNCH_INTERVAL).await;
                    continue;
                }
            };

            match self.node.connect_using_socket(peer_addr, socket).await {
                Ok(()) => {
                    info!(
                        parent: self.node.span(),
                        "P2P connection to {peer_addr} established on attempt {attempt}",
                    );
                    let greeting = format!("hello from {}", self.name);
                    let _ = self.unicast(peer_addr, Bytes::from(greeting));
                    return;
                }
                Err(e) => {
                    if attempt == HOLE_PUNCH_ATTEMPTS {
                        warn!(
                            parent: self.node.span(),
                            "hole punching to {peer_addr} failed after {attempt} attempts: {e}",
                        );
                    } else {
                        debug!(
                            parent: self.node.span(),
                            "attempt {attempt} to {peer_addr} failed: {e}; retrying...",
                        );
                        sleep(HOLE_PUNCH_INTERVAL).await;
                    }
                }
            }
        }
    }
}

impl Reading for Peer {
    type Message = BytesMut;
    type Codec = LengthDelimitedCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        LengthDelimitedCodec::new()
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) {
        if source == self.rendezvous_addr {
            // signaling from the rendezvous
            let text = match std::str::from_utf8(&message) {
                Ok(t) => t,
                Err(_) => {
                    warn!(parent: self.node.span(), "ignoring non-UTF8 signal from {source}");
                    return;
                }
            };
            let Some((tag, name, addr)) = parse_signal(text) else {
                warn!(parent: self.node.span(), "ignoring malformed signal from {source}: {text:?}");
                return;
            };
            if tag != "peer" {
                warn!(parent: self.node.span(), "unexpected signal tag from rendezvous: {tag:?}");
                return;
            }
            info!(parent: self.node.span(), "rendezvous paired us with {name} at {addr}");
            self.punch_to(addr).await;
            // signaling is done; drop the rendezvous connection so traffic is
            // purely P2P from here on
            self.node.disconnect(self.rendezvous_addr).await;
        } else {
            // P2P message from the other peer, over the punched connection
            let text = String::from_utf8_lossy(&message);
            info!(parent: self.node.span(), "📩 P2P from {source}: \"{text}\"");
        }
    }
}

impl Writing for Peer {
    type Message = Bytes;
    type Codec = LengthDelimitedCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        LengthDelimitedCodec::new()
    }
}

// === Entry point ===

#[tokio::main]
async fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    let program = args.first().map(String::as_str).unwrap_or("hole_punching");

    match args.get(1).map(String::as_str) {
        Some("rendezvous") => {
            let bind: SocketAddr = args
                .get(2)
                .map(|s| {
                    s.parse()
                        .unwrap_or_else(|_| panic!("invalid address: {s:?}"))
                })
                .unwrap_or_else(|| DEFAULT_RENDEZVOUS.parse().unwrap());
            run_rendezvous(bind).await
        }
        Some(name @ ("alice" | "bob")) => {
            let rendezvous: SocketAddr = args
                .get(2)
                .map(|s| {
                    s.parse()
                        .unwrap_or_else(|_| panic!("invalid address: {s:?}"))
                })
                .unwrap_or_else(|| DEFAULT_RENDEZVOUS.parse().unwrap());
            run_peer(name.to_string(), rendezvous).await
        }
        _ => {
            eprintln!("Usage:");
            eprintln!(
                "  {program} rendezvous [addr]   - start the signaling server (default {DEFAULT_RENDEZVOUS})"
            );
            eprintln!(
                "  {program} <alice|bob> [addr]  - start a peer (default rendezvous: {DEFAULT_RENDEZVOUS})"
            );
            eprintln!();
            eprintln!("Run one rendezvous and both peers to watch them punch a hole and talk.");
            std::process::exit(1);
        }
    }
}

async fn run_rendezvous(bind: SocketAddr) -> io::Result<()> {
    examples::start_logger(LevelFilter::INFO);

    let server = Rendezvous::new(bind);
    server.enable_reading().await;
    server.enable_writing().await;
    let bound = server
        .node()
        .toggle_listener()
        .await?
        .expect("listener bind");
    info!(parent: server.node().span(), "rendezvous listening on {bound}");

    std::future::pending::<()>().await;
    #[allow(unreachable_code)]
    Ok(())
}

async fn run_peer(name: String, rendezvous_addr: SocketAddr) -> io::Result<()> {
    examples::start_logger(LevelFilter::INFO);

    // Reserve the port we'll punch from *before* enabling protocols, so the
    // value is baked into the struct that the reading task clones.
    let punch_addr = reserve_punch_addr()?;
    info!("{name} will punch from {punch_addr}");

    let peer = Peer::new(name.clone(), rendezvous_addr, punch_addr);
    peer.enable_reading().await;
    peer.enable_writing().await;
    // NB: no `toggle_listener` - see the module docs.

    // connect to the rendezvous (an ordinary outbound connection)
    peer.node().connect(rendezvous_addr).await?;
    sleep(POST_CONNECT_SETTLE).await;

    // announce our punch address (in a real deployment: the STUN-learned one)
    peer.unicast(rendezvous_addr, encode_register(&name, punch_addr))
        .map_err(|e| io::Error::other(format!("couldn't register with rendezvous: {e}")))?;
    info!(parent: peer.node().span(), "registered with the rendezvous; waiting to be paired");

    // everything from here on happens in the Reading handler
    std::future::pending::<()>().await;
    #[allow(unreachable_code)]
    Ok(())
}
