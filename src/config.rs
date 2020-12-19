#[derive(Clone, Copy, Debug)]
pub enum ByteOrder {
    BE,
    LE,
}

#[derive(Clone, Debug)]
pub struct NodeConfig {
    /// message byte order
    pub byte_order: ByteOrder,
    /// number of bytes containing the size of messages
    pub message_length_size: u8,
    /// the size of a per-connection buffer for incoming messages
    pub conn_read_buffer_size: usize,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            byte_order: ByteOrder::BE,
            message_length_size: 4,
            conn_read_buffer_size: 64 * 1024,
        }
    }
}
