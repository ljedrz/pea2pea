//! A simple Request/Response (RPC) implementation over raw TCP.
//!
//! Demonstrates how to correlate requests with responses using IDs and oneshot channels,
//! using standard tokio codecs without intermediate wrappers.

mod common;

use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use bytes::{Buf, BufMut, Bytes, BytesMut};
use parking_lot::Mutex;
use pea2pea::{
    Config, ConnectionSide, Node, Pea2Pea,
    protocols::{Reading, Writing},
};
use tokio::sync::oneshot;
use tokio_util::codec::LengthDelimitedCodec;
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

type RequestId = u64;

enum Message {
    Request { id: RequestId, data: String },
    Response { id: RequestId, data: String },
}

impl Message {
    // serializes to a vector. We will convert this to `Bytes` for sending
    fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        match self {
            Message::Request { id, data } => {
                buf.put_u8(0); // type 0
                buf.put_u64(*id);
                buf.put_slice(data.as_bytes());
            }
            Message::Response { id, data } => {
                buf.put_u8(1); // type 1
                buf.put_u64(*id);
                buf.put_slice(data.as_bytes());
            }
        }
        buf
    }

    // deserializes directly from the BytesMut provided by the codec
    fn decode(mut buf: BytesMut) -> Option<Self> {
        // there must be at least the type and the id
        if buf.len() < 9 {
            return None;
        }

        let msg_type = buf.get_u8();
        let id = buf.get_u64();
        let data = String::from_utf8(Vec::from(buf)).ok()?;

        match msg_type {
            0 => Some(Message::Request { id, data }),
            1 => Some(Message::Response { id, data }),
            _ => None,
        }
    }
}

#[derive(Clone)]
struct RpcNode {
    node: Node,
    next_request_id: Arc<AtomicU64>,
    pending_requests: Arc<Mutex<HashMap<RequestId, oneshot::Sender<String>>>>,
}

impl Pea2Pea for RpcNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

// 1. WRITING: We use standard Bytes, which LengthDelimitedCodec knows how to encode
impl Writing for RpcNode {
    type Message = Bytes;
    type Codec = LengthDelimitedCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        LengthDelimitedCodec::new()
    }
}

// 2. READING: LengthDelimitedCodec yields BytesMut, which we parse manually
impl Reading for RpcNode {
    type Message = BytesMut;
    type Codec = LengthDelimitedCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        LengthDelimitedCodec::new()
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) {
        let Some(msg) = Message::decode(message) else {
            return;
        };

        match msg {
            Message::Request { id, data } => {
                info!(parent: self.node().span(), "handling request #{id}: \"{data}\"");
                let reply = format!("processed: {}", data.to_uppercase());

                // send response
                let response = Message::Response { id, data: reply };
                let bytes = Bytes::from(response.encode());
                let _ = self.unicast_fast(source, bytes);
            }
            Message::Response { id, data } => {
                // wake up the caller
                if let Some(tx) = self.pending_requests.lock().remove(&id) {
                    let _ = tx.send(data);
                }
            }
        }
    }
}

impl RpcNode {
    fn new(name: &str) -> Self {
        let config = Config {
            name: Some(name.into()),
            ..Default::default()
        };
        Self {
            node: Node::new(config),
            next_request_id: Default::default(),
            pending_requests: Default::default(),
        }
    }

    async fn rpc(&self, target: SocketAddr, data: &str) -> io::Result<String> {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();

        self.pending_requests.lock().insert(id, tx);

        let msg = Message::Request {
            id,
            data: data.into(),
        };
        // convert Vec<u8> -> Bytes for the Writing trait
        let bytes = Bytes::from(msg.encode());

        if let Err(e) = self.unicast_fast(target, bytes) {
            self.pending_requests.lock().remove(&id);
            return Err(e);
        }

        // wait for response
        let response = tokio::time::timeout(Duration::from_secs(1), rx)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "RPC timed out"))?
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Sender dropped"))?;

        Ok(response)
    }
}

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::INFO);

    let server = RpcNode::new("server");
    server.enable_reading().await;
    server.enable_writing().await;
    let server_addr = server.node().toggle_listener().await.unwrap().unwrap();

    let client = RpcNode::new("client");
    client.enable_reading().await;
    client.enable_writing().await;
    client.node().connect(server_addr).await.unwrap();

    info!(parent: client.node().span(), "sending request...");
    match client.rpc(server_addr, "hello world").await {
        Ok(res) => info!(parent: client.node().span(), "RPC success! Server said: \"{}\"", res),
        Err(e) => error!(parent: client.node().span(), "RPC failed: {}", e),
    }

    // cleanup
    client.node().shut_down().await;
    server.node().shut_down().await;
}
