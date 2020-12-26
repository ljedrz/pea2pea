use parking_lot::RwLock;
use tokio::task::JoinHandle;

use crate::connection::Connection;

use bytes::Bytes;
use fxhash::FxHashMap;
use std::{
    io::{self, ErrorKind},
    net::SocketAddr,
    sync::Arc,
};

type ConnectionMap = FxHashMap<SocketAddr, Arc<Connection>>;

#[derive(Default)]
pub(crate) struct Connections {
    pub(crate) handshaking: RwLock<ConnectionMap>,
    pub(crate) handshaken: RwLock<ConnectionMap>,
}

impl Connections {
    pub(crate) fn is_connected(&self, addr: SocketAddr) -> bool {
        self.is_handshaking(addr) || self.is_handshaken(addr)
    }

    pub(crate) fn is_handshaking(&self, addr: SocketAddr) -> bool {
        self.handshaking.read().contains_key(&addr)
    }

    pub(crate) fn is_handshaken(&self, addr: SocketAddr) -> bool {
        self.handshaken.read().contains_key(&addr)
    }

    pub(crate) fn disconnect(&self, addr: SocketAddr) -> bool {
        if self.handshaking.write().remove(&addr).is_none() {
            self.handshaken.write().remove(&addr).is_some()
        } else {
            true
        }
    }

    pub(crate) fn num_connected(&self) -> usize {
        self.handshaking.read().len() + self.handshaken.read().len()
    }

    pub(crate) fn handshaken_connections(&self) -> Vec<Arc<Connection>> {
        self.handshaken.read().values().cloned().collect()
    }

    pub(crate) async fn mark_as_handshaken(
        &self,
        addr: SocketAddr,
        reader_task: Option<JoinHandle<()>>,
    ) -> io::Result<()> {
        if let Some(conn) = self.handshaking.write().remove(&addr) {
            if let Some(task) = reader_task {
                conn.reader_task.set(task).unwrap();
            }
            self.handshaken.write().insert(addr, conn);
            Ok(())
        } else {
            Err(io::ErrorKind::NotConnected.into())
        }
    }

    pub(crate) async fn send_direct_message(
        &self,
        target: SocketAddr,
        message: Bytes,
    ) -> io::Result<()> {
        let conn = self.handshaken.read().get(&target).cloned();

        if let Some(ref conn) = conn {
            conn.send_message(message).await;
            Ok(())
        } else {
            Err(ErrorKind::NotConnected.into())
        }
    }
}
