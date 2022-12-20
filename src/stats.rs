use std::{
    sync::atomic::{AtomicU64, Ordering::Relaxed},
    time::Instant,
};

/// Contains basic statistics related to a node or a connection.
pub struct Stats {
    /// The creation time.
    created: Instant,
    /// The number of all messages sent.
    msgs_sent: AtomicU64,
    /// The number of all messages received.
    msgs_received: AtomicU64,
    /// The number of all bytes sent.
    bytes_sent: AtomicU64,
    /// The number of all bytes received.
    bytes_received: AtomicU64,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            created: Instant::now(),
            msgs_sent: Default::default(),
            msgs_received: Default::default(),
            bytes_sent: Default::default(),
            bytes_received: Default::default(),
        }
    }
}

impl Stats {
    /// Registers a sent message of the provided `size` in bytes.
    pub fn register_sent_message(&self, size: usize) {
        self.msgs_sent.fetch_add(1, Relaxed);
        self.bytes_sent.fetch_add(size as u64, Relaxed);
    }

    /// Registers a received message of the provided `size` in bytes.
    pub fn register_received_message(&self, size: usize) {
        self.msgs_received.fetch_add(1, Relaxed);
        self.bytes_received.fetch_add(size as u64, Relaxed);
    }

    /// Returns the creation time.
    pub fn created(&self) -> Instant {
        self.created
    }

    /// Returns the number of sent messages and their collective size in bytes.
    pub fn sent(&self) -> (u64, u64) {
        let msgs = self.msgs_sent.load(Relaxed);
        let bytes = self.bytes_sent.load(Relaxed);

        (msgs, bytes)
    }

    /// Returns the number of received messages and their collective size in bytes.
    pub fn received(&self) -> (u64, u64) {
        let msgs = self.msgs_received.load(Relaxed);
        let bytes = self.bytes_received.load(Relaxed);

        (msgs, bytes)
    }
}
