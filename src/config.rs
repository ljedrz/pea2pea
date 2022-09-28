use std::{
    io::{self, ErrorKind::*},
    net::{IpAddr, Ipv4Addr},
};

#[cfg(doc)]
use crate::protocols::{self, Handshake, Reading, Writing};

/// The node's configuration. See the source of [`Config::default`] for the defaults.
#[derive(Debug, Clone)]
pub struct Config {
    /// A user-friendly identifier of the node. It is visible in the logs, where it allows nodes to be
    /// distinguished more easily if multiple are run at the same time.
    ///
    /// note: If set to `None`, the node will automatically be assigned a sequential, zero-based numeric identifier.
    pub name: Option<String>,
    /// The IP address the node's connection listener should bind to.
    ///
    /// note: If set to `None`, the node will not listen for inbound connections at all.
    pub listener_ip: Option<IpAddr>,
    /// The desired listening port of the node. If [`Config::allow_random_port`] is set to `true`, the node
    /// will attempt to bind its listener to a different port if the desired one is not available.
    ///
    /// note: [`Config::listener_ip`] must not be `None` in order for it to have any effect.
    pub desired_listening_port: Option<u16>,
    /// Allow listening on a different port if [`Config::desired_listening_port`] is unavailable.
    ///
    /// note: [`Config::listener_ip`] must not be `None` in order for it to have any effect.
    pub allow_random_port: bool,
    /// The list of IO errors considered fatal and causing the connection to be dropped.
    ///
    /// note: The node needs to implement the [`Reading`] and/or [`Writing`] protocol in order for it to have any effect.
    pub fatal_io_errors: Vec<io::ErrorKind>,
    /// The maximum number of active connections the node can maintain at any given time.
    ///
    /// note: This number can very briefly be breached by 1 in case of inbound connection attempts. It can never be
    /// breached by outbound connection attempts, though.
    pub max_connections: u16,
}

impl Default for Config {
    fn default() -> Self {
        #[cfg(feature = "test")]
        fn default_ip() -> Option<IpAddr> {
            Some(IpAddr::V4(Ipv4Addr::LOCALHOST))
        }

        #[cfg(not(feature = "test"))]
        fn default_ip() -> Option<IpAddr> {
            Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
        }

        Self {
            name: None,
            listener_ip: default_ip(),
            desired_listening_port: None,
            allow_random_port: true,
            fatal_io_errors: vec![
                ConnectionReset,
                ConnectionAborted,
                BrokenPipe,
                InvalidData,
                UnexpectedEof,
            ],
            max_connections: 100,
        }
    }
}
