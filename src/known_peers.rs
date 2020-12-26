use parking_lot::RwLock;

use std::{collections::HashMap, net::SocketAddr, time::Instant};

#[derive(Default)]
pub struct KnownPeers(RwLock<HashMap<SocketAddr, PeerStats>>);

impl KnownPeers {
    pub(crate) fn add(&self, addr: SocketAddr) {
        self.0.write().entry(addr).or_default().new_connection();
    }

    pub(crate) fn register_received_message(&self, from: SocketAddr, len: usize) {
        self.0
            .write()
            .get_mut(&from)
            .unwrap()
            .register_received_message(len)
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

    pub fn peer_stats(&self) -> &RwLock<HashMap<SocketAddr, PeerStats>> {
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
    pub(crate) fn new_connection(&mut self) {
        self.last_seen = Instant::now();
        self.times_connected += 1;
    }

    pub(crate) fn register_received_message(&mut self, msg_len: usize) {
        self.last_seen = Instant::now();
        self.msgs_received += 1;
        self.bytes_received += msg_len as u64;
    }

    pub(crate) fn register_failure(&mut self) {
        self.last_seen = Instant::now();
        self.failures += 1;
    }
}
