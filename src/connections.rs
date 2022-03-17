//! Objects associated with connection handling.

#[cfg(doc)]
use crate::protocols::Handshake;

use parking_lot::RwLock;
use tokio::{
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpStream,
    },
    task::JoinHandle,
};

use std::{collections::HashMap, net::SocketAddr, ops::Not};

#[derive(Default)]
pub(crate) struct Connections(RwLock<HashMap<SocketAddr, Connection>>);

impl Connections {
    pub(crate) fn add(&self, conn: Connection) {
        self.0.write().insert(conn.addr, conn);
    }

    pub(crate) fn is_connected(&self, addr: SocketAddr) -> bool {
        self.0.read().contains_key(&addr)
    }

    pub(crate) fn remove(&self, addr: SocketAddr) -> Option<Connection> {
        self.0.write().remove(&addr)
    }

    pub(crate) fn num_connected(&self) -> usize {
        self.0.read().len()
    }

    pub(crate) fn addrs(&self) -> Vec<SocketAddr> {
        self.0.read().keys().copied().collect()
    }
}

/// Indicates who was the initiator and who was the responder when the connection was established.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionSide {
    /// The side that initiated the connection.
    Initiator,
    /// The sider that accepted the connection.
    Responder,
}

impl Not for ConnectionSide {
    type Output = Self;

    fn not(self) -> Self::Output {
        match self {
            Self::Initiator => Self::Responder,
            Self::Responder => Self::Initiator,
        }
    }
}

/// Created for each active connection; used by the protocols to obtain a handle for
/// reading and writing, and keeps track of tasks that have been spawned for the purposes
/// of the connection.
pub struct Connection {
    /// The address of the connection.
    pub addr: SocketAddr,
    /// Kept only until the protocols are enabled (the reading protocol should take it).
    pub(crate) reader: Option<OwnedReadHalf>,
    /// Kept only until the protocols are enabled (the writing protocol should take it).
    pub(crate) writer: Option<OwnedWriteHalf>,
    /// Handles to tasks spawned for the connection.
    pub(crate) tasks: Vec<JoinHandle<()>>,
    /// The connection's side in relation to the node.
    pub side: ConnectionSide,
}

impl Connection {
    /// Creates a [`Connection`] with placeholders for protocol-related objects.
    pub(crate) fn new(addr: SocketAddr, stream: TcpStream, side: ConnectionSide) -> Self {
        let (reader, writer) = stream.into_split();

        Self {
            addr,
            reader: Some(reader),
            writer: Some(writer),
            side,
            tasks: Default::default(),
        }
    }

    /// Obtains the full underlying stream; can be useful for handshake purposes. The stream must
    /// be returned with [`Connection::return_stream`] before the end of [`Handshake::perform_handshake`].
    pub fn take_stream(&mut self) -> TcpStream {
        self.reader
            .take()
            .expect("Connection's reader is not available!")
            .reunite(
                self.writer
                    .take()
                    .expect("Connection's writer is not available!"),
            )
            .unwrap()
    }

    /// Restores the stream obtained via [`Connection::take_stream`].
    pub fn return_stream(&mut self, stream: TcpStream) {
        let (reader, writer) = stream.into_split();
        self.reader = Some(reader);
        self.writer = Some(writer);
    }

    /// Provides mutable access to the underlying reader; it should only be used in protocol definitions.
    pub fn reader(&mut self) -> &mut OwnedReadHalf {
        self.reader
            .as_mut()
            .expect("Connection's reader is not available!")
    }

    /// Provides mutable access to the underlying writer; it should only be used in protocol definitions.
    pub fn writer(&mut self) -> &mut OwnedWriteHalf {
        self.writer
            .as_mut()
            .expect("Connection's writer is not available!")
    }
}
