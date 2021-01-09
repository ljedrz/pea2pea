use std::sync::atomic::{AtomicU64, Ordering};

/// Contains statistics related to the node.
#[derive(Default)]
pub struct NodeStats {
    /// The number of connections the node has established during its lifetime.
    connections: AtomicU64,
    /// The number of all messages sent.
    msgs_sent: AtomicU64,
    /// The number of all messages received.
    msgs_received: AtomicU64,
    /// The number of all bytes sent.
    bytes_sent: AtomicU64,
    /// The number of all bytes received.
    bytes_received: AtomicU64,
}

impl NodeStats {
    /// Registers an established connection.
    pub fn register_connection(&self) {
        self.connections.fetch_add(1, Ordering::Relaxed);
    }

    /// Registers a sent message of the provided `size` in bytes.
    pub fn register_sent_message(&self, size: usize) {
        self.msgs_sent.fetch_add(1, Ordering::Relaxed);
        self.bytes_sent.fetch_add(size as u64, Ordering::Relaxed);
    }

    /// Registers a received message of the provided `size` in bytes.
    pub fn register_received_message(&self, size: usize) {
        self.msgs_received.fetch_add(1, Ordering::Relaxed);
        self.bytes_received
            .fetch_add(size as u64, Ordering::Relaxed);
    }

    /// Returns the number of sent messages and their collective size in bytes.
    pub fn sent(&self) -> (u64, u64) {
        let msgs = self.msgs_sent.load(Ordering::Relaxed);
        let bytes = self.bytes_sent.load(Ordering::Relaxed);

        (msgs, bytes)
    }

    /// Returns the number of received messages and their collective size in bytes.
    pub fn received(&self) -> (u64, u64) {
        let msgs = self.msgs_received.load(Ordering::Relaxed);
        let bytes = self.bytes_received.load(Ordering::Relaxed);

        (msgs, bytes)
    }
}
