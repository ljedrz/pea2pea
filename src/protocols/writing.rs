use crate::{
    connections::{Connection, ConnectionWriter},
    Pea2Pea,
};

use async_trait::async_trait;
use bytes::Bytes;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tracing::*;

use std::io;

/// Can be used to specify and enable writing, i.e. sending outbound messages.
/// If handshaking is enabled too, it goes into force only after the handshake has been concluded.
#[async_trait]
pub trait Writing: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    // TODO: add an associated type defaulting to ConnectionWriter once
    // https://github.com/rust-lang/rust/issues/29661 is resolved.

    /// Prepares the node to send messages.
    fn enable_writing(&self) {
        let (conn_sender, mut conn_receiver) =
            mpsc::channel::<WritingObjects>(self.node().config.writing_handler_queue_depth);

        // the task spawning tasks reading messages from the given stream
        let self_clone = self.clone();
        let writing_task = tokio::spawn(async move {
            trace!(parent: self_clone.node().span(), "spawned the `Writing` handler task");

            loop {
                // these objects are sent from `Node::adapt_stream`
                if let Some((mut conn_writer, mut conn, conn_returner)) = conn_receiver.recv().await
                {
                    let addr = conn.addr;

                    let (outbound_message_sender, mut outbound_message_receiver) =
                        mpsc::channel::<Bytes>(self_clone.node().config.conn_outbound_queue_depth);

                    // the task for writing outbound messages
                    let writer_clone = self_clone.clone();
                    let writer_task = tokio::spawn(async move {
                        let node = writer_clone.node();
                        trace!(parent: node.span(), "spawned a task for writing messages to {}", addr);

                        loop {
                            // TODO: when try_recv is available in tokio again (https://github.com/tokio-rs/tokio/issues/3350),
                            // try adding a buffer for extra writing perf
                            while let Some(msg) = outbound_message_receiver.recv().await {
                                if let Err(e) =
                                    writer_clone.write_message(&mut conn_writer, &msg).await
                                {
                                    node.known_peers().register_failure(addr);
                                    error!(parent: node.span(), "couldn't send {}B to {}: {}", msg.len(), addr, e);
                                } else {
                                    node.known_peers().register_sent_message(addr, msg.len());
                                    node.stats.register_sent_message(msg.len());
                                    trace!(parent: node.span(), "sent {}B to {}", msg.len(), addr);
                                }
                            }
                        }
                    });

                    // the Connection object registers the handle of the newly created task and
                    // the Sender that will allow the Node to transmit messages to it
                    conn.tasks.push(writer_task);
                    conn.outbound_message_sender = Some(outbound_message_sender);

                    // return the Connection to the Node, resuming Node::adapt_stream
                    if conn_returner.send(Ok(conn)).is_err() {
                        // can't recover if this happens
                        panic!("can't return a Connection to the Node");
                    }
                }
            }
        });

        // register the WritingHandler with the Node
        self.node()
            .set_writing_handler((conn_sender, writing_task).into());
    }

    /// Writes the provided bytes to the connection's stream.
    async fn write_message(&self, writer: &mut ConnectionWriter, payload: &[u8]) -> io::Result<()>;
}

/// A set of objects required to enable the `Writing` protocol.
pub type WritingObjects = (
    ConnectionWriter,
    Connection,
    oneshot::Sender<io::Result<Connection>>,
);

/// An object dedicated to spawning outbound message handlers; used in the `Writing` protocol.
pub struct WritingHandler {
    sender: mpsc::Sender<WritingObjects>,
    _task: JoinHandle<()>,
}

impl WritingHandler {
    /// Sends writing-relevant objects to the task spawned by the WritingHandler.
    pub async fn send(&self, writing_objects: WritingObjects) {
        if self.sender.send(writing_objects).await.is_err() {
            // can't recover if this happens
            panic!("WritingHandler's Receiver is closed")
        }
    }
}

impl From<(mpsc::Sender<WritingObjects>, JoinHandle<()>)> for WritingHandler {
    fn from((sender, _task): (mpsc::Sender<WritingObjects>, JoinHandle<()>)) -> Self {
        Self { sender, _task }
    }
}
