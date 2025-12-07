use std::{future::Future, io, time::Duration};

use tokio::{
    io::{AsyncRead, AsyncWrite, split},
    net::TcpStream,
    sync::{mpsc, oneshot},
    task::JoinSet,
    time::timeout,
};
use tracing::*;

use crate::{
    Connection, Pea2Pea,
    node::NodeTask,
    protocols::{ProtocolHandler, ReturnableConnection},
};

/// Can be used to specify and enable network handshakes, and to configure the sockets. Upon establishing
/// a connection, both sides will need to adhere to the specified handshake rules in order to finalize
/// the connection and be able to send or receive any messages.
pub trait Handshake: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// The maximum time allowed for a connection to perform a handshake before it is rejected.
    const TIMEOUT_MS: u64 = 3_000;

    /// Prepares the node to perform specified network handshakes.
    fn enable_handshake(&self) -> impl Future<Output = ()> + Send {
        async {
            // create a JoinSet to track all in-flight setup tasks
            let mut setup_tasks = JoinSet::new();

            let (conn_sender, mut conn_receiver) =
                mpsc::channel::<ReturnableConnection>(self.node().config().max_connecting as usize);

            // use a channel to know when the handshake task is ready
            let (tx, rx) = oneshot::channel();

            // spawn a background task dedicated to handling the handshakes
            let self_clone = self.clone();
            let handshake_task = tokio::spawn(async move {
                trace!(parent: self_clone.node().span(), "spawned the Handshake handler task");
                if tx.send(()).is_err() {
                    error!(parent: self_clone.node().span(), "Handshake handler creation interrupted! shutting down the node");
                    self_clone.node().shut_down().await;
                    return;
                }

                loop {
                    tokio::select! {
                        // handle new connections from `Node::adapt_stream`
                        maybe_conn = conn_receiver.recv() => {
                            match maybe_conn {
                                Some(returnable_conn) => {
                                    let self_clone2 = self_clone.clone();
                                    setup_tasks.spawn(async move {
                                        self_clone2.handle_new_connection(returnable_conn).await;
                                    });
                                }
                                None => break, // channel closed
                            }
                        }
                        // task set cleanups
                        _ = setup_tasks.join_next(), if !setup_tasks.is_empty() => {}
                    }
                }
            });
            let _ = rx.await;
            self.node()
                .tasks
                .lock()
                .insert(NodeTask::Handshake, handshake_task);

            // register the Handshake handler with the Node
            let hdl = ProtocolHandler(conn_sender);
            assert!(
                self.node().protocols.handshake.set(hdl).is_ok(),
                "the Handshake protocol was enabled more than once!"
            );
        }
    }

    /// Performs the handshake; temporarily assumes control of the [`Connection`] and returns it if the handshake is
    /// successful.
    ///
    /// Since it provides access to the underlying [`TcpStream`] (via `Handshake::borrow_stream`),
    /// this is the appropriate place to configure socket options such as `TCP_NODELAY`,
    /// `SO_KEEPALIVE`, or buffer sizes (`SO_RCVBUF` / `SO_SNDBUF`).
    fn perform_handshake(
        &self,
        conn: Connection,
    ) -> impl Future<Output = io::Result<Connection>> + Send;

    /// Borrows the full connection stream to be used in the implementation of [`Handshake::perform_handshake`].
    fn borrow_stream<'a>(&self, conn: &'a mut Connection) -> &'a mut TcpStream {
        conn.stream.as_mut().unwrap()
    }

    /// Assumes full control of a connection's stream in the implementation of [`Handshake::perform_handshake`], by
    /// the end of which it *must* be followed by [`Handshake::return_stream`].
    fn take_stream(&self, conn: &mut Connection) -> TcpStream {
        conn.stream.take().unwrap()
    }

    /// This method only needs to be called if [`Handshake::take_stream`] had been called before; it is used to
    /// return a (potentially modified) stream back to the applicable connection.
    fn return_stream<T: AsyncRead + AsyncWrite + Send + Sync + 'static>(
        &self,
        conn: &mut Connection,
        stream: T,
    ) {
        let (reader, writer) = split(stream);
        conn.reader = Some(Box::new(reader));
        conn.writer = Some(Box::new(writer));
    }
}

trait HandshakeInternal: Handshake {
    /// Applies the [`Handshake`] protocol to a single connection.
    fn handle_new_connection(
        &self,
        conn_with_returner: ReturnableConnection,
    ) -> impl Future<Output = ()> + Send;
}

impl<H: Handshake> HandshakeInternal for H {
    async fn handle_new_connection(&self, (conn, conn_returner): ReturnableConnection) {
        let conn_span = conn.span().clone();

        debug!(parent: &conn_span, "executing Handshake logic...");
        let result = timeout(
            Duration::from_millis(Self::TIMEOUT_MS),
            self.perform_handshake(conn),
        )
        .await;

        let ret = match result {
            Ok(Ok(conn)) => {
                debug!(parent: &conn_span, "handshake succeeded");
                Ok(conn)
            }
            Ok(Err(e)) => {
                error!(parent: &conn_span, "handshake failed: {e}");
                Err(e)
            }
            Err(_) => {
                error!(parent: &conn_span, "handshake timed out");
                Err(io::ErrorKind::TimedOut.into())
            }
        };

        // return the Connection to the Node, resuming Node::adapt_stream
        if conn_returner.send(ret).is_err() {
            error!(parent: conn_span, "couldn't return a Connection from the Handshake handler");
        }
    }
}
