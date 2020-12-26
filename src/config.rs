#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// the name/identifier of the node
    pub name: Option<String>,
    /// the desired listening port of the node
    pub desired_listening_port: Option<u16>,
    /// allow listening on a different port if desired_listening_port is unavailable
    pub allow_random_port: bool,
    /// the size of a per-connection buffer for inbound messages
    pub conn_read_buffer_size: usize,
    /// the depth of the queue used to process all inbound messages
    pub inbound_message_queue_depth: usize,
    /// the depth of the per-connection queues used to send messages
    pub outbound_message_queue_depth: usize,
    /// the delay on the next read from a node that provided an invalid message
    pub invalid_message_penalty_secs: u64,
    /// the per-connection number of failures over which the connection is closed
    pub max_allowed_failures: u8,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            name: None,
            desired_listening_port: None,
            allow_random_port: true,
            conn_read_buffer_size: 64 * 1024,
            inbound_message_queue_depth: 256,
            outbound_message_queue_depth: 16,
            invalid_message_penalty_secs: 10,
            max_allowed_failures: 10,
        }
    }
}
