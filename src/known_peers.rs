use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use fxhash::FxHashMap;
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed},
        Arc,
    },
};

/// Contains statistics related to node's peers, currently connected or not.
#[derive(Default)]
pub struct KnownPeers(RwLock<FxHashMap<SocketAddr, Arc<PeerStats>>>);

impl KnownPeers {
    /// Adds an address to the list of known peers.
    pub fn add(&self, addr: SocketAddr) {
        self.write().entry(addr).or_default();
    }

    /// Removes an address to the list of known peers.
    pub fn remove(&self, addr: SocketAddr) -> Option<Arc<PeerStats>> {
        self.write().remove(&addr)
    }

    /// Registers a connection to the given address.
    pub fn register_connection(&self, addr: SocketAddr) {
        if let Some(stats) = self.read().get(&addr) {
            stats.times_connected.fetch_add(1, Relaxed);
        }
    }

    /// Registers a submission of a message to the given address.
    pub fn register_sent_message(&self, to: SocketAddr, len: usize) {
        if let Some(stats) = self.read().get(&to) {
            stats.msgs_sent.fetch_add(1, Relaxed);
            stats.bytes_sent.fetch_add(len as u64, Relaxed);
        }
    }

    /// Registers a receipt of a message to the given address.
    pub fn register_received_message(&self, from: SocketAddr, len: usize) {
        if let Some(stats) = self.read().get(&from) {
            stats.msgs_received.fetch_add(1, Relaxed);
            stats.bytes_received.fetch_add(len as u64, Relaxed);
        }
    }

    /// Registers a failure associated with the given address.
    pub fn register_failure(&self, addr: SocketAddr) {
        if let Some(stats) = self.read().get(&addr) {
            stats.failures.fetch_add(1, Relaxed);
        }
    }

    /// Acquires a read lock over the collection of known peers.
    pub fn read(&self) -> RwLockReadGuard<'_, FxHashMap<SocketAddr, Arc<PeerStats>>> {
        self.0.read()
    }

    /// Acquires a write lock over the collection of known peers.
    pub fn write(&self) -> RwLockWriteGuard<'_, FxHashMap<SocketAddr, Arc<PeerStats>>> {
        self.0.write()
    }
}

/// Contains statistics related to a single peer.
#[derive(Debug, Default)]
pub struct PeerStats {
    /// The number of times a connection with the peer has been established.
    pub times_connected: AtomicUsize,
    /// The number of messages sent to the peer.
    pub msgs_sent: AtomicUsize,
    /// The number of messages received from the peer.
    pub msgs_received: AtomicUsize,
    /// The number of bytes sent to the peer.
    pub bytes_sent: AtomicU64,
    /// The number of bytes received from the peer.
    pub bytes_received: AtomicU64,
    /// The number of failures related to the peer.
    pub failures: AtomicUsize,
}
