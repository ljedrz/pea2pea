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
    pub listener_addr: Option<SocketAddr>,
    /// The maximum number of active connections the node can maintain at any given time.
    ///
    /// note: For accuracy and performance, the pending connections are also included when checking
    /// this limit - it is assumed that they may all conclude successfully.
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
    pub max_connections_per_ip: u16,
    /// The maximum number of simultaneous connection attempts (a.k.a. pending connections).
    ///
    /// note: It should not be greater than `max_connections`, as it will clash with it.
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
