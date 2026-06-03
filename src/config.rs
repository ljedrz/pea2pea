use std::net::{IpAddr, Ipv4Addr, SocketAddr};

#[cfg(doc)]
use crate::{
    Node,
    protocols::{self, Handshake, Reading, Writing},
};

/// The node's configuration. See the source of [`Config::default`] for the defaults.
#[derive(Debug, Clone)]
pub struct Config {
    /// A user-friendly identifier of the node. It is visible in the logs, where it allows nodes to be
    /// distinguished more easily if multiple are run at the same time.
    ///
    /// note: If set to `None`, the node will automatically be assigned a sequential, zero-based numeric identifier.
    pub name: Option<String>,
    /// The socket address the node's connection listener should bind to.
    ///
    /// note: If set to `None`, the node will not listen for inbound connections at all.
    ///
    /// note: This is a configuration directive, not a live status. If set to `127.0.0.1:0`,
    /// the actual bound port will differ. Always use [`Node::listening_addr`] to retrieve the
    /// active runtime address.
    ///
    /// note: Binding to a wildcard address (e.g. `0.0.0.0`) means the library cannot reliably
    /// detect a self-connect attempt to one of the host's other local addresses. See the note
    /// on [`Node::connect`] for details.
    pub listener_addr: Option<SocketAddr>,
    /// The depth of the OS accept queue (the `backlog` argument to `listen(2)`): how many
    /// fully-established inbound connections the kernel will hold awaiting `accept` before it
    /// stops admitting new ones.
    ///
    /// note: This value is silently capped by the OS to `net.core.somaxconn` on Linux
    /// (`kern.ipc.somaxconn` on macOS/BSD). The effective depth is `min(listener_backlog, somaxconn)`,
    /// so raising it past the system limit does nothing until that limit is raised as well.
    ///
    /// note: When the queue is full the kernel does not, by default, actively refuse the connection -
    /// it silently drops the handshake completion and the peer retransmits on the TCP timer, so the
    /// symptom is a connection that appears to stall for seconds rather than one that fails fast.
    /// (On Linux, `net.ipv4.tcp_abort_on_overflow=1` turns these into immediate resets instead.)
    ///
    /// note: For graceful backpressure under a burst, keep this at least on the order of
    /// [`Config::max_connecting`]: inbound setup is throttled to `max_connecting` at a time, and the
    /// backlog is what holds the surplus while that pipeline drains. A backlog much smaller than
    /// `max_connecting` lets bursts overflow before setup catches up.
    pub listener_backlog: u32,
    /// The maximum number of active connections the node can maintain at any given time.
    ///
    /// note: For accuracy and performance, the pending connections are also included when checking
    /// this limit - it is assumed that they may all conclude successfully.
    ///
    /// note: As a `u16`, a single node tops out at 65,535 connections; scaling beyond that means
    /// distributing load across multiple nodes/listeners.
    pub max_connections: u16,
    /// The maximum number of active connections the node can maintain with a single IP.
    ///
    /// note: Like [`Config::max_connections`], pending connections are also included in the
    /// related calculations.
    ///
    /// note: This limit matches the exact IP address. It does not aggregate subnets. An attacker
    /// with an IPv6 `/64` subnet can bypass this limit by assigning a unique address for every
    /// connection (up to [`Config::max_connections`]). If you expose your node to the public IPv6
    /// internet, rely on the global connection limit for resource protection, or implement an
    /// application-level subnet filter in [`Handshake`].
    ///
    /// note: Conversely, when many connections legitimately share one source IP - load tests, a NAT
    /// gateway, a reverse proxy, or anything over loopback - raise this to match, or those peers are
    /// rejected once the per-IP count is hit. Note the default outside the `test` feature is 1.
    pub max_connections_per_ip: u16,
    /// The maximum number of simultaneous connection attempts (a.k.a. pending connections), covering
    /// both outbound connects in progress and inbound connections still being accepted and handshaked.
    ///
    /// note: It should not be greater than [`Config::max_connections`]: pending connections count
    /// towards that limit, so a value above it can never actually be reached.
    ///
    /// note: On the inbound path this doubles as a backpressure bound - at most `max_connecting`
    /// inbound connections are set up concurrently, and any surplus waits in the OS accept queue
    /// (see [`Config::listener_backlog`], which should be sized accordingly).
    pub max_connecting: u16,
    /// The maximum time (in milliseconds) allowed to establish a raw (before the [`Handshake`] protocol) TCP connection.
    pub connection_timeout_ms: u16,
    /// Determines whether duplicate connections to the same target address are allowed.
    pub allow_duplicate_connections: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            name: None,
            #[cfg(not(feature = "test"))]
            listener_addr: Some((IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0).into()),
            #[cfg(feature = "test")]
            listener_addr: Some((IpAddr::V4(Ipv4Addr::LOCALHOST), 0).into()),
            listener_backlog: 128,
            max_connections: 100,
            #[cfg(not(feature = "test"))]
            max_connections_per_ip: 1,
            #[cfg(feature = "test")]
            max_connections_per_ip: 100,
            max_connecting: 100,
            connection_timeout_ms: 1_000,
            allow_duplicate_connections: false,
        }
    }
}
