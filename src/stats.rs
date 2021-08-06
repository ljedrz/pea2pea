use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Contains statistics related to a node.
#[derive(Default)]
pub struct Stats {
    /// The number of all messages sent.
    msgs_sent: AtomicU64,
    /// The number of all messages received.
    msgs_received: AtomicU64,
    /// The number of all bytes sent.
    bytes_sent: AtomicU64,
    /// The number of all bytes received.
    bytes_received: AtomicU64,
    /// The number of failures.
    failures: AtomicU64,
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

    /// Registers a failure.
    pub fn register_failure(&self) {
        self.failures.fetch_add(1, Relaxed);
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

    /// Returns the number of failures.
    pub fn failures(&self) -> u64 {
        self.failures.load(Relaxed)
    }
}
