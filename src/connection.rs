use crate::Node;

use bytes::Bytes;
use once_cell::sync::OnceCell;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::tcp::{OwnedReadHalf, OwnedWriteHalf},
    sync::mpsc::{channel, Sender},
    task::JoinHandle,
};
use tracing::*;

use std::{
    io::{self, ErrorKind},
    net::SocketAddr,
    ops::Not,
    sync::Arc,
};

/// Indicates who was the initiator and who was the responder when the connection was established.
#[derive(Clone, Copy)]
pub(crate) enum ConnectionSide {
    Initiator,
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

/// An object dedicated to performing reads from a connection's stream.
pub struct ConnectionReader {
    /// A reference to the owning node.
    pub node: Arc<Node>,
    /// The address of the connection.
    peer_addr: SocketAddr,
    /// A buffer dedicated to reading from the stream.
    pub buffer: Box<[u8]>,
    /// The read half of the stream.
    pub reader: OwnedReadHalf,
}

impl ConnectionReader {
    pub(crate) fn new(peer_addr: SocketAddr, reader: OwnedReadHalf, node: Arc<Node>) -> Self {
        Self {
            buffer: vec![0; node.config.conn_read_buffer_size].into(),
            node,
            peer_addr,
            reader,
        }
    }

    /// Reads as many bytes as there are queued to be read from the stream.
    pub async fn read_queued_bytes(&mut self) -> io::Result<&[u8]> {
        let len = self.reader.read(&mut self.buffer).await?;
        trace!(parent: self.node.span(), "read {}B from {}", len, self.peer_addr);

        Ok(&self.buffer[..len])
    }

    /// Reads the specified number of bytes from the stream.
    pub async fn read_exact(&mut self, num: usize) -> io::Result<&[u8]> {
        let buffer = &mut self.buffer;

        if num > buffer.len() {
            error!(parent: self.node.span(), "can' read {}B from the stream; the buffer is too small ({}B)", num, buffer.len());
            return Err(ErrorKind::Other.into());
        }

        self.reader.read_exact(&mut buffer[..num]).await?;
        trace!(parent: self.node.span(), "read {}B from {}", num, self.peer_addr);

        Ok(&buffer[..num])
    }
}

/// An object dedicated to performing writes to the stream
pub struct Connection {
    /// A reference to the owning node.
    node: Arc<Node>,
    /// The address of the connection.
    peer_addr: SocketAddr,
    /// The handle to the task performing reads from the stream.
    pub(crate) reader_task: OnceCell<JoinHandle<()>>,
    /// The handle to the task performing writes to the stream.
    _writer_task: JoinHandle<()>,
    /// Used to queue writes to the stream.
    message_sender: Sender<Bytes>,
    /// The connection's side in relation to the node.
    side: ConnectionSide,
}

impl Connection {
    pub(crate) fn new(
        peer_addr: SocketAddr,
        mut writer: OwnedWriteHalf,
        node: Arc<Node>,
        side: ConnectionSide,
    ) -> Self {
        let (message_sender, mut message_receiver) =
            channel::<Bytes>(node.config.outbound_message_queue_depth);

        let node_clone = Arc::clone(&node);
        let _writer_task = tokio::spawn(async move {
            loop {
                // TODO: when try_recv is available in tokio again (https://github.com/tokio-rs/tokio/issues/3350),
                // try adding a buffer for extra writing perf
                while let Some(msg) = message_receiver.recv().await {
                    if let Err(e) = writer.write_all(&msg).await {
                        node_clone.register_failure(peer_addr);
                        error!(parent: node_clone.span(), "couldn't send {}B to {}: {}", msg.len(), peer_addr, e);
                    } else {
                        node_clone.register_sent_message(peer_addr, msg.len());
                        trace!(parent: node_clone.span(), "sent {}B to {}", msg.len(), peer_addr);
                    }
                }
                if let Err(e) = writer.flush().await {
                    node_clone.register_failure(peer_addr);
                    error!(parent: node_clone.span(), "couldn't flush the stream to {}: {}", peer_addr, e);
                }
            }
        });
        trace!(parent: node.span(), "spawned a task for writing messages to {}", peer_addr);

        Self {
            node,
            peer_addr,
            reader_task: Default::default(),
            _writer_task,
            message_sender,
            side,
        }
    }

    /// Sends the given message to the peer associated with the connection.
    pub async fn send_message(&self, message: Bytes) {
        // can't recover if this happens
        self.message_sender
            .send(message)
            .await
            .expect("the connection writer task is closed");
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        debug!(parent: self.node.span(), "disconnecting from {}", self.peer_addr);
        // if the (owning) node was not the initiator of the connection, it doesn't know the listening address
        // of the associated peer, so the related stats are unreliable; the next connection initiated by the
        // peer could be bound to an entirely different port number
        if matches!(self.side, ConnectionSide::Initiator) {
            self.node
                .known_peers
                .peer_stats()
                .write()
                .remove(&self.peer_addr);
        }
    }
}
