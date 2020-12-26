use parking_lot::RwLock;
use tokio::task::JoinHandle;

use crate::connection::Connection;

use std::{
    collections::HashMap,
    io::{self, ErrorKind},
    net::SocketAddr,
    sync::Arc,
};

type ConnectionMap = HashMap<SocketAddr, Arc<Connection>>;

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

    pub(crate) fn handshaken_connections(&self) -> Vec<(SocketAddr, Arc<Connection>)> {
        self.handshaken
            .read()
            .iter()
            .map(|(addr, conn)| (*addr, Arc::clone(conn)))
            .collect()
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
        header: Option<&[u8]>,
        payload: &[u8],
    ) -> io::Result<()> {
        let conn = self.handshaken.read().get(&target).cloned();

        let conn = if conn.is_some() {
            conn
        } else {
            return Err(ErrorKind::NotConnected.into());
        };

        if let Some(ref conn) = conn {
            conn.send_message(header, payload).await
        } else {
            Err(io::ErrorKind::NotConnected.into())
        }
    }
}
