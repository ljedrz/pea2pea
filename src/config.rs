#[cfg(doc)]
use crate::protocols::{self, Handshake, Reading, Writing};

use std::{
    io::{self, ErrorKind::*},
    net::{IpAddr, Ipv4Addr},
};

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
    /// The depth of the queues passing connections to protocol handlers.
    ///
    /// note: The node needs to implement at least one of the [`protocols`] for it to have any effect.
    pub protocol_handler_queue_depth: usize,
    /// The delay on the next read attempt from a connection that can't be read from.
    ///
    /// note: The node needs to implement the [`Reading`] protocol in order for it to have any effect.
    pub invalid_read_delay_secs: u64,
    /// The list of IO errors considered fatal and causing the connection to be dropped.
    ///
    /// note: The node needs to implement the [`Reading`] and/or [`Writing`] protocol in order for it to have any effect.
    pub fatal_io_errors: Vec<io::ErrorKind>,
    /// The maximum number of active connections the node can maintain at any given time.
    ///
    /// note: This number can very briefly be breached by 1 in case of inbound connection attempts. It can never be
    /// breached by outbound connection attempts, though.
    pub max_connections: u16,

    /// The size of a per-connection buffer for reading inbound messages. It should be at least as large as the
    /// maximum message size permitted by the network. Any inbound message larger than this value will be rejected
    /// and cause a disconnect (unless [`Config::fatal_io_errors`] is altered to exclude [`InvalidData`]).
    ///
    /// note: The node needs to implement the [`Reading`] protocol in order for it to have any effect.
    pub read_buffer_size: usize,
    /// The depth of per-connection queues used to process inbound messages; the greater it is, the more inbound
    /// messages the node can enqueue, but setting it to a large value can make the node more susceptible to DoS
    /// attacks.
    ///
    /// note: The node needs to implement the [`Reading`] protocol in order for it to have any effect.
    pub inbound_queue_depth: usize,
    /// The depth of per-connection queues used to send outbound messages; the greater it is, the more outbound
    /// messages the node can enqueue. Setting it to a large value is not recommended, as doing it might
    /// obscure potential issues with your implementation (like slow serialization) or network.
    ///
    /// note: The node needs to implement the [`Writing`] protocol in order for it to have any effect.
    pub outbound_queue_depth: usize,
    /// The maximum time allowed for a connection to perform a handshake before it is rejected.
    ///
    /// note: The node needs to implement the [`Handshake`] protocol in order for it to have any effect.
    pub max_handshake_time_ms: u64,
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
            protocol_handler_queue_depth: 16,
            invalid_read_delay_secs: 10,
            fatal_io_errors: vec![
                ConnectionReset,
                ConnectionAborted,
                BrokenPipe,
                InvalidData,
                UnexpectedEof,
            ],
            max_connections: 100,

            read_buffer_size: 64 * 1024,
            inbound_queue_depth: 64,
            outbound_queue_depth: 64,
            max_handshake_time_ms: 3_000,
        }
    }
}
