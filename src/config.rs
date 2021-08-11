use std::{
    io::{self, ErrorKind::*},
    net::{IpAddr, Ipv4Addr},
};

/// The node's configuration.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// The name/identifier of the node.
    ///
    /// note: if set to `None`, the `Node` will automatically be assigned a sequential, zero-based numeric identifier.
    pub name: Option<String>,
    /// The IP address the node's connection listener should bind to.
    ///
    /// note: if set to `None`, the `Node` will not listen for inbound connections at all.
    pub listener_ip: Option<IpAddr>,
    /// The desired listening port of the node.
    pub desired_listening_port: Option<u16>,
    /// Allow listening on a different port if `desired_listening_port` is unavailable.
    pub allow_random_port: bool,
    /// The depth of the queues passing connections to protocol handlers.
    pub protocol_handler_queue_depth: usize,
    /// The delay on the next read attempt from a connection that can't be read from.
    pub invalid_read_delay_secs: u64,
    /// The list of IO errors considered fatal and causing the connection to be dropped.
    pub fatal_io_errors: Vec<io::ErrorKind>,
    /// The maximum number of active connections the node can maintain.
    ///
    /// note: this number can very briefly be breached by 1 in case of inbound connection attempts. It can never be
    /// breached by outbound connection attempts, though.
    pub max_connections: u16,

    /// The size of a per-connection buffer for reading inbound messages.
    pub read_buffer_size: usize,
    /// The size of a per-connection buffer for writing outbound messages.
    pub write_buffer_size: usize,
    /// The depth of per-connection queues used to process inbound messages.
    pub inbound_queue_depth: usize,
    /// The depth of per-connection queues used to send outbound messages.
    pub outbound_queue_depth: usize,
    /// The maximum time allowed for a connection to perform a handshake before it is rejected.
    pub max_handshake_time_ms: u64,
}

impl Default for NodeConfig {
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
            write_buffer_size: 64 * 1024,
            inbound_queue_depth: 64,
            outbound_queue_depth: 64,
            max_handshake_time_ms: 3_000,
        }
    }
}
