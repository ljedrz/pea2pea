use crate::{
    protocols::{ProtocolHandler, ReturnableConnection},
    Node, Pea2Pea,
};

#[cfg(doc)]
use crate::{protocols::Handshake, Config};

use async_trait::async_trait;
use bytes::BytesMut;
use futures_util::StreamExt;
use tokio::{
    io::AsyncRead,
    sync::{mpsc, oneshot},
    time::sleep,
};
use tokio_util::codec::{Decoder, FramedRead};
use tracing::*;

use std::{io, net::SocketAddr, time::Duration};

/// Can be used to specify and enable reading, i.e. receiving inbound messages. If the [`Handshake`]
/// protocol is enabled too, it goes into force only after the handshake has been concluded.
#[async_trait]
pub trait Reading: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// The final (deserialized) type of inbound messages.
    type Message: Send;

    /// The user-supplied [`Decoder`] used to interpret the inbound messages.
    type Codec: Decoder<Item = Self::Message, Error = io::Error> + Send;

    /// Prepares the node to receive messages; failures to read from a connection's stream are penalized by a timeout
    /// defined in [`Config`], while the configured fatal errors result in an immediate disconnect (in order to e.g. avoid
    /// accidentally reading "borked" messages).
    async fn enable_reading(&self) {
        let (conn_sender, mut conn_receiver) = mpsc::unbounded_channel::<ReturnableConnection>();

        // Use a channel to know when the reading task is ready.
        let (tx_reading, rx_reading) = oneshot::channel::<()>();

        // the main task spawning per-connection tasks reading messages from their streams
        let self_clone = self.clone();
        let reading_task = tokio::spawn(async move {
            trace!(parent: self_clone.node().span(), "spawned the Reading handler task");
            tx_reading.send(()).unwrap(); // safe; the channel was just opened

            // these objects are sent from `Node::adapt_stream`
            while let Some((mut conn, conn_returner)) = conn_receiver.recv().await {
                let addr = conn.addr();
                let codec = self_clone.codec(addr);
                let reader = conn.reader.take().unwrap();
                let framed = FramedRead::new(reader, codec);
                let mut framed = self_clone.map_codec(framed, addr);

                let initial_read_buffer_size = self_clone.node().config().initial_read_buffer_size;
                if initial_read_buffer_size != 0 {
                    framed.read_buffer_mut().reserve(initial_read_buffer_size);
                }

                let (inbound_message_sender, mut inbound_message_receiver) =
                    mpsc::channel(self_clone.node().config().inbound_queue_depth);

                // Use a channel to know when the processing task is ready.
                let (tx_processing, rx_processing) = oneshot::channel::<()>();

                // the task for processing parsed messages
                let processing_clone = self_clone.clone();
                let inbound_processing_task = tokio::spawn(async move {
                    let node = processing_clone.node();
                    trace!(parent: node.span(), "spawned a task for processing messages from {}", addr);
                    tx_processing.send(()).unwrap(); // safe; the channel was just opened

                    while let Some(msg) = inbound_message_receiver.recv().await {
                        if let Err(e) = processing_clone.process_message(addr, msg).await {
                            error!(parent: node.span(), "can't process a message from {}: {}", addr, e);
                            node.known_peers().register_failure(addr);
                        }
                    }
                });
                let _ = rx_processing.await;
                conn.tasks.push(inbound_processing_task);

                // Use a channel to know when the reader task is ready.
                let (tx_reader, rx_reader) = oneshot::channel::<()>();

                // the task for reading messages from a stream
                let node = self_clone.node().clone();
                let reader_task = tokio::spawn(async move {
                    trace!(parent: node.span(), "spawned a task for reading messages from {}", addr);
                    tx_reader.send(()).unwrap(); // safe; the channel was just opened

                    // postpone reads until the connection is fully established; if the process fails,
                    // this task gets aborted, so there is no need for a dedicated timeout
                    while !node.connected_addrs().contains(&addr) {
                        sleep(Duration::from_millis(1)).await;
                    }

                    while let Some(bytes) = framed.next().await {
                        match bytes {
                            Ok(msg) => {
                                // send the message for further processing
                                if let Err(e) = inbound_message_sender.try_send(msg) {
                                    error!(parent: node.span(), "can't process a message from {}: {}", addr, e);
                                    node.stats().register_failure();
                                }
                            }
                            Err(e) => {
                                error!(parent: node.span(), "can't read from {}: {}", addr, e);
                                node.known_peers().register_failure(addr);
                                if node.config().fatal_io_errors.contains(&e.kind()) {
                                    node.disconnect(addr).await;
                                    break;
                                }
                            }
                        }
                    }

                    let _ = node.disconnect(addr).await;
                });
                let _ = rx_reader.await;
                conn.tasks.push(reader_task);

                // return the Connection to the Node, resuming Node::adapt_stream
                if conn_returner.send(Ok(conn)).is_err() {
                    unreachable!("could't return a Connection to the Node");
                }
            }
        });
        let _ = rx_reading.await;
        self.node().tasks.lock().push(reading_task);

        // register the Reading handler with the Node
        let hdl = Box::new(ProtocolHandler(conn_sender));
        assert!(
            self.node().protocols.reading_handler.set(hdl).is_ok(),
            "the Reading protocol was enabled more than once!"
        );
    }

    /// Wraps the user-supplied [`Decoder`] in another one used for message accounting.
    #[doc(hidden)]
    fn map_codec<T: AsyncRead>(
        &self,
        framed: FramedRead<T, Self::Codec>,
        addr: SocketAddr,
    ) -> FramedRead<T, CountingCodec<Self::Codec>> {
        framed.map_decoder(|codec| CountingCodec {
            codec,
            node: self.node().clone(),
            addr,
            acc: 0,
        })
    }

    /// Creates a [`Decoder`] used to interpret messages from the network.
    fn codec(&self, _addr: SocketAddr) -> Self::Codec;

    /// Processes an inbound message. Can be used to update state, send replies etc.
    async fn process_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()>;
}

/// A wrapper [`Decoder`] that also counts the inbound messages.
#[doc(hidden)]
pub struct CountingCodec<D: Decoder> {
    codec: D,
    node: Node,
    addr: SocketAddr,
    acc: usize,
}

impl<D: Decoder> Decoder for CountingCodec<D> {
    type Item = D::Item;
    type Error = D::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let initial_buf_len = src.len();
        let ret = self.codec.decode(src)?;
        let final_buf_len = src.len();
        let read_len = initial_buf_len - final_buf_len + self.acc;

        if read_len != 0 {
            trace!(parent: self.node.span(), "read {}B from {}", read_len, self.addr);

            if ret.is_some() {
                self.acc = 0;
                self.node
                    .known_peers()
                    .register_received_message(self.addr, read_len);
                self.node.stats().register_received_message(read_len);
            } else {
                self.acc = read_len;
            }
        }

        Ok(ret)
    }
}
