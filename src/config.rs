#[derive(Clone, Copy, Debug)]
pub enum ByteOrder {
    BE,
    LE,
}

#[derive(Debug)]
pub struct NodeConfig {
    /// the name/identifier of the node
    pub name: Option<String>,
    /// the desired listening port of the node
    pub desired_listening_port: Option<u16>,
    /// allow listening on a different port if desired_listening_port is unavailable
    pub allow_random_port: bool,
    /// message byte order
    pub message_byte_order: ByteOrder,
    /// number of bytes containing the size of messages
    pub message_length_size: u8,
    /// the size of a per-connection buffer for incoming messages
    pub conn_read_buffer_size: usize,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            name: None,
            desired_listening_port: None,
            allow_random_port: true,
            message_byte_order: ByteOrder::BE,
            message_length_size: 4,
            conn_read_buffer_size: 64 * 1024,
        }
    }
}
