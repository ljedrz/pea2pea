mod common;

use bytes::{Bytes, BytesMut};
use native_tls::Identity;
use tokio::time::sleep;
use tokio_native_tls::{TlsAcceptor, TlsConnector};
use tokio_util::codec::BytesCodec;
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

use pea2pea::{
    protocols::{Handshake, Reading, Writing},
    Config, Connection, ConnectionSide, Node, Pea2Pea,
};

use std::{io, net::SocketAddr, time::Duration};

#[derive(Clone)]
struct TlsNode {
    node: Node,
    acceptor: TlsAcceptor,
    connector: TlsConnector,
}

impl TlsNode {
    async fn new<T: Into<String>>(name: T) -> Self {
        // node config
        let config = Config {
            name: Some(name.into()),
            ..Default::default()
        };

        // TLS acceptor (a single hardcoded identity for simplicity)
        let file = include_bytes!("../examples/common/tls_identity.pfx");
        let identity = Identity::from_pkcs12(&file[..], "pass").unwrap();
        let inner_acceptor = native_tls::TlsAcceptor::builder(identity).build().unwrap();
        let acceptor = TlsAcceptor::from(inner_acceptor);

        // TLS connector
        let inner_connector = native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();
        let connector = TlsConnector::from(inner_connector);

        Self {
            node: Node::new(Some(config)).await.unwrap(),
            acceptor,
            connector,
        }
    }
}

impl Pea2Pea for TlsNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

#[async_trait::async_trait]
impl Handshake for TlsNode {
    async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
        let node_conn_side = !conn.side();
        let stream = self.take_stream(&mut conn);

        let tls_stream = match node_conn_side {
            ConnectionSide::Initiator => self
                .connector
                .connect("localhost", stream)
                .await
                .map_err(|e| {
                    error!(parent: self.node().span(), "TLS handshake error: {}", e);
                    io::ErrorKind::InvalidData
                })?,
            ConnectionSide::Responder => self.acceptor.accept(stream).await.map_err(|e| {
                error!(parent: self.node().span(), "TLS handshake error: {}", e);
                io::ErrorKind::InvalidData
            })?,
        };

        self.return_stream(&mut conn, tls_stream);

        Ok(conn)
    }
}

#[async_trait::async_trait]
impl Reading for TlsNode {
    type Message = BytesMut;
    type Codec = BytesCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()> {
        info!(parent: self.node().span(), "read some bytes from {}: {:?}", source, message);

        Ok(())
    }
}

impl Writing for TlsNode {
    type Message = Bytes;
    type Codec = BytesCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::TRACE);

    // start the TLS-capable nodes; note: both can initiate and accept connections
    let connector = TlsNode::new("connector").await;
    let acceptor = TlsNode::new("acceptor").await;

    for node in &[&connector, &acceptor] {
        node.enable_handshake().await;
        node.enable_reading().await;
        node.enable_writing().await;
    }

    // connect the connector to the acceptor
    connector
        .node()
        .connect(acceptor.node().listening_addr().unwrap())
        .await
        .unwrap();

    // determine the connector's address first
    sleep(Duration::from_millis(10)).await;
    let connector_addr = acceptor.node().connected_addrs()[0];

    // prepare a generic message
    let msg = Bytes::from(b"herp derp".to_vec());

    // send a message from connector to acceptor
    let _ = connector
        .send_direct_message(acceptor.node().listening_addr().unwrap(), msg.clone())
        .unwrap()
        .await;

    // send a message from acceptor to connector
    let _ = acceptor
        .send_direct_message(connector_addr, msg)
        .unwrap()
        .await;

    // a small delay to ensure all messages were processed
    sleep(Duration::from_millis(10)).await;
}
