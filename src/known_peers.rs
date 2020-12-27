use parking_lot::RwLock;

use fxhash::FxHashMap;
use std::{net::SocketAddr, time::Instant};

#[derive(Default)]
pub struct KnownPeers(RwLock<FxHashMap<SocketAddr, PeerStats>>);

impl KnownPeers {
    pub(crate) fn add(&self, addr: SocketAddr) {
        self.0.write().entry(addr).or_default().new_connection();
    }

    pub(crate) fn update_last_seen(&self, addr: SocketAddr) {
        if let Some(ref mut stats) = self.0.write().get_mut(&addr) {
            stats.update_last_seen();
        }
    }

    pub(crate) fn register_received_message(&self, from: SocketAddr, len: usize) {
        if let Some(ref mut stats) = self.0.write().get_mut(&from) {
            stats.register_received_message(len)
        }
    }

    pub(crate) fn register_failure(&self, from: SocketAddr) {
        if let Some(ref mut stats) = self.0.write().get_mut(&from) {
            stats.register_failure();
        }
    }

    pub(crate) fn num_messages_received(&self) -> usize {
        self.0
            .read()
            .values()
            .map(|peer_stats| peer_stats.msgs_received)
            .sum()
    }

    pub fn peer_stats(&self) -> &RwLock<FxHashMap<SocketAddr, PeerStats>> {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct PeerStats {
    pub times_connected: usize,
    pub first_seen: Instant,
    pub last_seen: Instant,
    pub msgs_received: usize,
    pub bytes_received: u64,
    pub failures: u8, // FIXME: consider some public "reset" method instead
}

impl Default for PeerStats {
    fn default() -> Self {
        let now = Instant::now();

        Self {
            times_connected: 1,
            first_seen: now,
            last_seen: now,
            msgs_received: 0,
            bytes_received: 0,
            failures: 0,
        }
    }
}

impl PeerStats {
    pub(crate) fn update_last_seen(&mut self) {
        // last_seen is not updated automatically, as many messages can be read
        // from the stream at once, and each one would cause a costly system call;
        // since it is a costly operation, it is opt-in; it can be included in
        // one of the methods provided by the Messaging protocol
        self.last_seen = Instant::now();
    }

    pub(crate) fn new_connection(&mut self) {
        self.times_connected += 1;
    }

    pub(crate) fn register_received_message(&mut self, msg_len: usize) {
        self.msgs_received += 1;
        self.bytes_received += msg_len as u64;
    }

    pub(crate) fn register_failure(&mut self) {
        self.failures += 1;
    }
}
