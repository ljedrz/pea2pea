use bytes::Bytes;
use parking_lot::RwLock;
use tokio::sync::mpsc::Sender;

use crate::connection::Connection;

use fxhash::FxHashMap;
use std::net::SocketAddr;

#[derive(Default)]
pub(crate) struct Connections(RwLock<FxHashMap<SocketAddr, Connection>>);

impl Connections {
    pub(crate) fn sender(&self, addr: SocketAddr) -> Option<Sender<Bytes>> {
        self.0
            .read()
            .get(&addr)
            .map(|conn| conn.message_sender.clone())
    }

    pub(crate) fn add(&self, conn: Connection) {
        self.0.write().insert(conn.addr, conn);
    }

    pub(crate) fn senders(&self) -> Vec<Sender<Bytes>> {
        self.0
            .read()
            .values()
            .map(|conn| conn.message_sender.clone())
            .collect()
    }

    pub(crate) fn is_connected(&self, addr: SocketAddr) -> bool {
        self.0.read().contains_key(&addr)
    }

    pub(crate) fn disconnect(&self, addr: SocketAddr) -> bool {
        self.0.write().remove(&addr).is_some()
    }

    pub(crate) fn num_connected(&self) -> usize {
        self.0.read().len()
    }

    pub(crate) fn addrs(&self) -> Vec<SocketAddr> {
        self.0.read().keys().copied().collect()
    }
}
