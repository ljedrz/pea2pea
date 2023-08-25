use std::{
    io::{self, ErrorKind::*},
    net::{IpAddr, Ipv4Addr, SocketAddr},
};

#[cfg(doc)]
use crate::{
    protocols::{self, Handshake, Reading, Writing},
    Node,
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
    pub listener_addr: Option<SocketAddr>,
    /// The list of IO errors considered fatal and causing the connection to be dropped.
    ///
    /// note: The node needs to implement the [`Reading`] and/or [`Writing`] protocol in order for it to have any effect.
    pub fatal_io_errors: Vec<io::ErrorKind>,
    /// The maximum number of active connections the node can maintain at any given time.
    ///
    /// note: This number can very briefly be breached by 1 in case of inbound connection attempts. It can never be
    /// breached by outbound connection attempts, though.
    pub max_connections: u16,
    /// The maximum time (in milliseconds) allowed to establish a raw (before the [`Handshake`] protocol) TCP connection.
    pub connection_timeout_ms: u16,
}

impl Default for Config {
    fn default() -> Self {
        #[cfg(feature = "test")]
        fn default_addr() -> Option<SocketAddr> {
            Some((IpAddr::V4(Ipv4Addr::LOCALHOST), 0).into())
        }

        #[cfg(not(feature = "test"))]
        fn default_addr() -> Option<SocketAddr> {
            Some((IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0).into())
        }

        Self {
            name: None,
            listener_addr: default_addr(),
            fatal_io_errors: vec![
                ConnectionReset,
                ConnectionAborted,
                BrokenPipe,
                InvalidData,
                UnexpectedEof,
            ],
            max_connections: 100,
            connection_timeout_ms: 1_000,
        }
    }
}
