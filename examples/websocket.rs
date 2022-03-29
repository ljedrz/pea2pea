mod common;

use bytes::BytesMut;
use futures_util::{future, sink::SinkExt, stream::TryStreamExt, StreamExt};
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    time::sleep,
};
use tokio_util::codec::{BytesCodec, Decoder, Encoder, Framed};
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

use pea2pea::{
    protocols::{Handshake, Reading, Writing},
    Connection, ConnectionSide, Node, Pea2Pea,
};

use std::{io, net::SocketAddr, time::Duration};

pub struct WsCodec(websocket_codec::MessageCodec);

impl Default for WsCodec {
    fn default() -> Self {
        // websocket_codec uses `true` for the client and `false` for the server
        WsCodec(websocket_codec::MessageCodec::with_masked_encode(true))
    }
}

impl Decoder for WsCodec {
    type Item = websocket_codec::Message;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.0
            .decode(src)
            .map_err(|_| io::ErrorKind::InvalidData.into())
    }
}

impl Encoder<Vec<u8>> for WsCodec {
    type Error = io::Error;

    fn encode(&mut self, item: Vec<u8>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let message = websocket_codec::Message::binary(item);
        self.0
            .encode(message, dst)
            .map_err(|_| io::ErrorKind::InvalidData.into())
    }
}

#[derive(Clone)]
struct WsNode {
    node: Node,
}

impl Pea2Pea for WsNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

#[async_trait::async_trait]
impl Handshake for WsNode {
    async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
        let conn_addr = conn.addr();
        let node_conn_side = !conn.side();
        let stream = self.borrow_stream(&mut conn);

        match node_conn_side {
            ConnectionSide::Initiator => {
                let mut framed = Framed::new(stream, BytesCodec::default());

                let mut request = Vec::new();
                request.extend_from_slice(b"GET / HTTP/1.1\r\n");
                request.extend_from_slice(format!("Host: {}\r\n", conn_addr).as_bytes());
                request.extend_from_slice(b"Connection: Upgrade\r\n");
                request.extend_from_slice(b"Upgrade: websocket\r\n");
                request.extend_from_slice(b"Sec-WebSocket-Version: 13\r\n");
                request.extend_from_slice(b"Sec-WebSocket-Key: nSYwkZhPRAatKmzjOM+6LQ==\r\n\r\n"); // TODO: un-hard-code

                trace!(parent: self.node().span(), "sending handshake message: b{:?}", std::str::from_utf8(&request).unwrap());

                framed.get_mut().write(&request).await.unwrap();

                let response = framed.try_next().await.unwrap().unwrap();

                trace!(parent: self.node().span(), "received handshake message: {:?}", response);
            }
            ConnectionSide::Responder => {
                let mut framed = Framed::new(stream, BytesCodec::default());

                let request = framed.next().await.unwrap().unwrap();

                trace!(parent: self.node().span(), "received handshake message: {:?}", request);

                let mut request_headers = [httparse::EMPTY_HEADER; 10];
                let mut parsed_request = httparse::Request::new(&mut request_headers);
                parsed_request.parse(&request).unwrap();

                let swa = if let Some(swk) = parsed_request
                    .headers
                    .iter()
                    .find(|h| h.name.to_ascii_lowercase() == "sec-websocket-key")
                {
                    tungstenite::handshake::derive_accept_key(&swk.value)
                } else {
                    error!(parent: self.node().span(), "missing Sec-WebSocket-Key!");
                    return Err(io::ErrorKind::InvalidData.into());
                };

                let mut response = Vec::new();
                response.extend_from_slice(b"HTTP/1.1 101 Switching Protocols\r\n");
                response.extend_from_slice(b"Connection: Upgrade\r\n");
                response.extend_from_slice(b"Upgrade: websocket\r\n");
                response
                    .extend_from_slice(format!("Sec-Websocket-Accept: {}\r\n\r\n", swa).as_bytes());

                trace!(parent: self.node().span(), "sending handshake message: b{:?}", std::str::from_utf8(&response).unwrap());

                framed.get_mut().write(&response).await.unwrap();
            }
        }

        Ok(conn)
    }
}

#[async_trait::async_trait]
impl Reading for WsNode {
    type Message = websocket_codec::Message;
    type Codec = WsCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()> {
        info!(parent: self.node().span(), "got a message from {}: {:?}", source, message);

        Ok(())
    }
}

impl Writing for WsNode {
    type Message = Vec<u8>;
    type Codec = WsCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::INFO);

    let ws_node = WsNode {
        node: Node::new(None).await.unwrap(),
    };
    ws_node.enable_handshake().await;
    ws_node.enable_reading().await;
    ws_node.enable_writing().await;

    // check comms with a WS server
    {
        let ws_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ws_addr = ws_listener.local_addr().unwrap();
        info!("Websocket server istening on {}", ws_addr);

        tokio::spawn(async move {
            while let Ok((stream, _)) = ws_listener.accept().await {
                tokio::spawn(accept_connection(stream));
            }
        });

        ws_node.node().connect(ws_addr).await.unwrap();
        info!(parent: ws_node.node().span(), "sending \\0 to {}", ws_addr);
        ws_node
            .send_direct_message(ws_addr, vec![0])
            .unwrap()
            .await
            .unwrap();

        sleep(Duration::from_millis(100)).await;
    }

    // comms with a WS client
    {
        let node_url = format!("ws://{}", ws_node.node().listening_addr().unwrap());
        let (mut ws_stream, _) = tokio_tungstenite::connect_async(&node_url)
            .await
            .expect("Failed to connect");

        info!("sending \\0 to {}", node_url);
        ws_stream
            .send(tungstenite::protocol::Message::Binary(vec![0]))
            .await
            .unwrap();

        sleep(Duration::from_millis(100)).await;
    }
}

async fn accept_connection(stream: TcpStream) {
    let addr = stream.peer_addr().unwrap();
    let ws_stream = tokio_tungstenite::accept_async(stream).await.unwrap();

    info!("New WebSocket connection: {}", addr);

    let (write, read) = ws_stream.split();
    // We should not forward messages other than text or binary.
    read.try_filter(|msg| future::ready(msg.is_text() || msg.is_binary()))
        .forward(write)
        .await
        .expect("Failed to forward messages")
}
