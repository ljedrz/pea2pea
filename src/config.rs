use std::net::{IpAddr, Ipv4Addr};

/// The node's configuration.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// The name/identifier of the node.
    ///
    /// note: if set to `None`, the `Node` will automatically be assigned a sequential, zero-based numeric identifier.
    pub name: Option<String>,
    /// The IP address the node's connection listener should bind to.
    pub listener_ip: IpAddr,
    /// The desired listening port of the node.
    pub desired_listening_port: Option<u16>,
    /// Allow listening on a different port if `desired_listening_port` is unavailable.
    pub allow_random_port: bool,
    /// The depth of the queues passing connections to protocol handlers.
    pub protocol_handler_queue_depth: usize,
    /// The size of a per-connection buffer for reading inbound messages.
    pub conn_read_buffer_size: usize,
    /// The size of a per-connection buffer for writing outbound messages.
    pub conn_write_buffer_size: usize,
    /// The depth of per-connection queues used to process inbound messages.
    pub conn_inbound_queue_depth: usize,
    /// The depth of per-connection queues used to send outbound messages.
    pub conn_outbound_queue_depth: usize,
    /// The delay on the next read from a connection that can't be read from.
    pub invalid_read_delay_secs: u64,
    /// The maximum number of active connections the node can maintain.
    ///
    /// note: this number can very briefly be breached by 1 in case of inbound connection attempts. It can never be
    /// breached by outbound connection attempts, though.
    pub max_connections: u16,
    /// The maximum time allowed for connections to enable all protocols.
    ///
    /// note: it should not be treated as a handshake timeout; the handshake is primarily intended to be an operation
    /// synchronous towards the `Connection` object: if a `Connection` is sent to the `Handshaking` protocol handler
    /// and the associated task hangs, it will not automatically get dropped and the task will not resume - it is the
    /// user's responsibility to handle it. The same warning applies to `Reading` and `Writing`, but they are intended
    /// to immediately spawn their own tasks (as in the default implementations), so they are less susceptible to this
    /// issue.
    pub max_protocol_setup_time_ms: u64,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            name: None,
            listener_ip: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            desired_listening_port: None,
            allow_random_port: true,
            protocol_handler_queue_depth: 16,
            conn_read_buffer_size: 64 * 1024,
            conn_write_buffer_size: 64 * 1024,
            conn_inbound_queue_depth: 256,
            conn_outbound_queue_depth: 16,
            invalid_read_delay_secs: 10,
            max_connections: 100,
            max_protocol_setup_time_ms: 3000,
        }
    }
}
