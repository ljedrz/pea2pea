use std::{future::Future, io, time::Duration};

use tokio::{
    io::{AsyncRead, AsyncWrite, split},
    net::TcpStream,
    sync::mpsc,
    time::timeout,
};
use tracing::*;

#[cfg(doc)]
use crate::Node;
use crate::{
    Connection, Pea2Pea,
    node::NodeTask,
    protocols::{
        ProtocolHandler, ReturnableConnection, install_protocol_handler, run_setup_handler_loop,
    },
};

/// Can be used to specify and enable network handshakes, and to configure the sockets. Upon establishing
/// a connection, both sides will need to adhere to the specified handshake rules in order to finalize
/// the connection and be able to send or receive any messages.
pub trait Handshake: Pea2Pea
where
    Self: Clone + Send + Sync + 'static,
{
    /// The maximum time allowed for a connection to perform a handshake before it is rejected.
    ///
    /// note: Unlike [`Reading::IDLE_TIMEOUT_MS`](crate::protocols::Reading::IDLE_TIMEOUT_MS),
    /// a value of `0` does not disable the timeout - it fails every handshake (almost) instantly.
    const TIMEOUT_MS: u64 = 3_000;

    /// Prepares the node to perform specified network handshakes.
    ///
    /// # Panics
    ///
    /// Panics if called more than once on the same [`Node`].
    fn enable_handshake(&self) -> impl Future<Output = ()> + Send {
        async {
            let (conn_sender, conn_receiver) =
                mpsc::channel::<ReturnableConnection>(self.node().config().max_connecting as usize);

            // spawn a background task dedicated to handling the handshakes
            let self_clone = self.clone();
            let handler_loop = async move {
                let node = self_clone.node().clone();
                run_setup_handler_loop(node, "Handshake", conn_receiver, |conn, setup_tasks| {
                    let self_clone = self_clone.clone();
                    setup_tasks.spawn(async move {
                        self_clone.handle_new_connection(conn).await;
                    });
                })
                .await;
            };

            install_protocol_handler(
                self.node(),
                NodeTask::Handshake,
                "Handshake",
                |protocols| &protocols.handshake,
                ProtocolHandler(conn_sender),
                handler_loop,
            )
            .await;
        }
    }

    /// Performs the handshake; temporarily assumes control of the [`Connection`] and returns it if the handshake is
    /// successful.
    ///
    /// note: Since it provides access to the underlying [`TcpStream`] (via [`Handshake::borrow_stream`]),
    /// this is the appropriate place to configure socket options such as `TCP_NODELAY`,
    /// `SO_KEEPALIVE`, or buffer sizes (`SO_RCVBUF` / `SO_SNDBUF`).
    fn perform_handshake(
        &self,
        conn: Connection,
    ) -> impl Future<Output = io::Result<Connection>> + Send;

    /// Borrows the full connection stream to be used in the implementation of [`Handshake::perform_handshake`].
    fn borrow_stream<'a>(&self, conn: &'a mut Connection) -> &'a mut TcpStream {
        conn.stream
            .as_mut()
            .expect("Stream not found; perhaps you've already called take_stream?")
    }

    /// Assumes full control of a connection's stream in the implementation of [`Handshake::perform_handshake`], by
    /// the end of which it *must* be followed by [`Handshake::return_stream`].
    fn take_stream(&self, conn: &mut Connection) -> TcpStream {
        conn.stream
            .take()
            .expect("Stream already taken; make sure take_stream is only called once")
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
                self.node().heuristics().register_handshake_timeout();
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
