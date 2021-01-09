/// The node's configuration.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// The name/identifier of the node.
    pub name: Option<String>,
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
    pub max_connections: u16,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            name: None,
            desired_listening_port: None,
            allow_random_port: true,
            protocol_handler_queue_depth: 16,
            conn_read_buffer_size: 64 * 1024,
            conn_write_buffer_size: 64 * 1024,
            conn_inbound_queue_depth: 256,
            conn_outbound_queue_depth: 16,
            invalid_read_delay_secs: 10,
            max_connections: 100,
        }
    }
}
