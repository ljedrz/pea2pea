/// The node's configuration.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// The name/identifier of the node.
    pub name: Option<String>,
    /// The desired listening port of the node.
    pub desired_listening_port: Option<u16>,
    /// Allow listening on a different port if desired_listening_port is unavailable.
    pub allow_random_port: bool,
    /// The size of a per-connection buffer for inbound messages.
    pub conn_read_buffer_size: usize,
    /// The depth of per-connection queues used to process inbound messages.
    pub conn_inbound_queue_depth: usize,
    /// The depth of per-connection queues used to send outbound messages.
    pub conn_outbound_queue_depth: usize,
    /// The delay on the next read from a node that provided an invalid message.
    pub invalid_message_penalty_secs: u64,
    /// The per-connection number of failures over which the connection is closed.
    pub max_allowed_failures: u8,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            name: None,
            desired_listening_port: None,
            allow_random_port: true,
            conn_read_buffer_size: 64 * 1024,
            conn_inbound_queue_depth: 256,
            conn_outbound_queue_depth: 16,
            invalid_message_penalty_secs: 10,
            max_allowed_failures: 10,
        }
    }
}
