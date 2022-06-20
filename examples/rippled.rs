mod common;

use common::ripple_protocol::*;

use bytes::{Buf, Bytes, BytesMut};
use futures_util::{sink::SinkExt, TryStreamExt};
use openssl::{
    pkcs12::Pkcs12,
    ssl::{Ssl, SslAcceptor, SslConnector, SslMethod, SslVerifyMode},
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use sha2::{Digest, Sha512};
use tokio_openssl::SslStream;
use tokio_util::codec::{BytesCodec, Decoder, Encoder, Framed};
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

use pea2pea::{
    protocols::{Handshake, Reading, Writing},
    Connection, ConnectionSide, Node, Pea2Pea,
};

use std::{io, net::SocketAddr, pin::Pin, sync::Arc};

const HEADER_LEN_COMPRESSED: u32 = 10;

const HEADER_LEN_UNCOMPRESSED: u32 = 6;

const COMPRESSION_ALGO: u8 = 0xf0;

const COMPRESSION_NONE: u8 = 0x00;

const COMPRESSION_LZ4: u8 = 0x90;

const COMPRESSED_TRUE: u8 = 0x80;

const COMPRESSED_FALSE: u8 = 0xfc;

const PROTOCOL_ERROR: u8 = 0x0c;

enum HttpMsg {
    Request,
    Response,
}

#[derive(Debug)]
struct Header {
    total_wire_size: u32,
    header_size: u32,
    payload_wire_size: u32,
    uncompressed_size: u32,
    message_type: u16,
    compression: Option<()>,
}

#[derive(Debug)]
#[non_exhaustive]
enum Payload {
    TmManifests(TmManifests),
    TmValidation(TmValidation),
    TmValidatorListCollection(TmValidatorListCollection),
    TmGetPeerShardInfoV2(TmGetPeerShardInfoV2),
}

#[derive(Debug)]
struct Message {
    header: Header,
    payload: Payload,
}

struct RippleCodec {
    codec: BytesCodec,
    current_msg_header: Option<Header>,
    // The associated node's span.
    span: Span,
}

impl RippleCodec {
    fn new(span: Span) -> Self {
        Self {
            codec: BytesCodec::default(),
            current_msg_header: None,
            span,
        }
    }
}

impl Decoder for RippleCodec {
    type Item = Message;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.is_empty() {
            return Ok(None);
        }

        if self.current_msg_header.is_none() {
            if src[0] & COMPRESSED_TRUE != 0 {
                trace!(parent: &self.span, "processing a compressed message");

                let header_size = HEADER_LEN_COMPRESSED;
                if src.remaining() < header_size as usize {
                    return Ok(None);
                }

                // protocol error
                if (src[0] & PROTOCOL_ERROR) != 0 {
                    unimplemented!();
                }

                let header_bytes = src.split_to(header_size as usize);
                let mut iter = header_bytes.into_iter();

                let compression = src[0] & COMPRESSION_ALGO;
                trace!(parent: &self.span, "compression: {:x}", compression);

                // only LZ4 is currently supported
                if compression != COMPRESSION_LZ4 {
                    unimplemented!();
                }

                let mut payload_wire_size = 0;
                for _ in 0..4 {
                    payload_wire_size = (payload_wire_size << 8u32) + iter.next().unwrap() as u32;
                }
                payload_wire_size &= 0x0FFFFFFF; // clear the top four bits (the compression bits)

                let total_wire_size = header_size + payload_wire_size;

                let mut message_type = 0;
                for _ in 0..2 {
                    message_type = (message_type << 8u16) + iter.next().unwrap() as u16;
                }

                let mut uncompressed_size = 0;
                for _ in 0..4 {
                    uncompressed_size = (uncompressed_size << 8u32) + iter.next().unwrap() as u32;
                }

                let header = Header {
                    total_wire_size,
                    header_size,
                    payload_wire_size,
                    uncompressed_size,
                    message_type,
                    compression: None,
                };

                self.current_msg_header = Some(header);
            } else if src[0] & COMPRESSED_FALSE == 0 {
                trace!(parent: &self.span, "processing an uncompressed message");

                let header_size = HEADER_LEN_UNCOMPRESSED;
                if src.remaining() < header_size as usize {
                    return Ok(None);
                }

                let header_bytes = src.split_to(header_size as usize);
                let mut iter = header_bytes.into_iter();

                let mut payload_wire_size = 0;
                for _ in 0..4 {
                    payload_wire_size = (payload_wire_size << 8u32) + iter.next().unwrap() as u32;
                }

                let uncompressed_size = payload_wire_size;
                let total_wire_size = header_size + payload_wire_size as u32;

                let mut message_type = 0;
                for _ in 0..2 {
                    message_type = (message_type << 8u16) + iter.next().unwrap() as u16;
                }

                let header = Header {
                    total_wire_size,
                    header_size,
                    payload_wire_size,
                    uncompressed_size,
                    message_type,
                    compression: None,
                };

                self.current_msg_header = Some(header);
            } else {
                error!(parent: &self.span, "invalid compression indicator");

                return Err(io::ErrorKind::InvalidData.into());
            }
        }

        if let Some(Header {
            payload_wire_size, ..
        }) = self.current_msg_header
        {
            if src.remaining() < payload_wire_size as usize {
                return Ok(None);
            }

            let header = self.current_msg_header.take().unwrap();
            let mut payload = src.split_to(payload_wire_size as usize);

            let payload = match header.message_type {
                2 => Payload::TmManifests(prost::Message::decode(&mut payload)?),
                41 => Payload::TmValidation(prost::Message::decode(&mut payload)?),
                56 => Payload::TmValidatorListCollection(prost::Message::decode(&mut payload)?),
                61 => Payload::TmGetPeerShardInfoV2(prost::Message::decode(&mut payload)?),
                _ => unimplemented!(),
            };

            let message = Message { header, payload };

            debug!(parent: &self.span, "decoded a header: {:?}", message.header);

            Ok(Some(message))
        } else {
            unreachable!();
        }
    }
}

impl Encoder<Message> for RippleCodec {
    type Error = io::Error;

    fn encode(&mut self, message: Message, dst: &mut BytesMut) -> Result<(), Self::Error> {
        todo!();
    }
}

// A codec used to handle HTTP messages.
struct HttpCodec {
    // The underlying codec.
    codec: BytesCodec,
    // The associated node's span.
    span: Span,
    // The next kind of HTTP message expected.
    expecting: HttpMsg,
}

impl HttpCodec {
    fn new(span: Span, expecting: HttpMsg) -> Self {
        HttpCodec {
            codec: Default::default(),
            span,
            expecting,
        }
    }
}

impl Decoder for HttpCodec {
    type Item = BytesMut;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let mut raw_bytes = if let Some(bytes) = self.codec.decode(src)? {
            bytes
        } else {
            return Ok(None);
        };

        trace!(parent: &self.span, "got some raw bytes: {:?}", raw_bytes);

        let mut headers = [httparse::EMPTY_HEADER; 16];

        let res = match self.expecting {
            HttpMsg::Request => {
                let mut req = httparse::Request::new(&mut headers);
                req.parse(&raw_bytes)
            }
            HttpMsg::Response => {
                let mut resp = httparse::Response::new(&mut headers);
                resp.parse(&raw_bytes)
            }
        }
        .map_err(|e| {
            error!(parent: &self.span, "HTTP parse error: {}", e);
            io::ErrorKind::InvalidData
        })?;

        match res {
            httparse::Status::Partial => {
                // TODO: check if openssl ensures the completeness of requests
                warn!(parent: &self.span, "unexpected partial HTTP response");
                Ok(None)
            }
            httparse::Status::Complete(header_length) => {
                raw_bytes.advance(header_length);

                Ok(Some(raw_bytes))
            }
        }
    }
}

impl Encoder<Bytes> for HttpCodec {
    type Error = io::Error;

    fn encode(&mut self, message: Bytes, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.codec.encode(message, dst)
    }
}

// An object dedicated to cryptographic functionalities.
struct Crypto {
    engine: Secp256k1<secp256k1::All>,
    private_key: SecretKey,
    public_key: PublicKey,
}

// An object cointaining TLS handlers.
#[derive(Clone)]
struct Tls {
    acceptor: SslAcceptor,
    connector: SslConnector,
}

// A TLS-capable node.
#[derive(Clone)]
struct TlsNode {
    node: Node,
    crypto: Arc<Crypto>,
    tls: Tls,
}

impl TlsNode {
    // Used during the handshake to populate the Session-Signature field.
    fn create_session_signature(&self, shared_value: &[u8]) -> String {
        let message = secp256k1::Message::from_slice(shared_value).unwrap();
        let signature = self
            .crypto
            .engine
            .sign_ecdsa(&message, &self.crypto.private_key);
        let serialized = signature.serialize_der();

        base64::encode(serialized)
    }
}

impl Pea2Pea for TlsNode {
    fn node(&self) -> &Node {
        &self.node
    }
}

// Used as input for create_session_signature.
fn get_shared_value<S>(tls_stream: &SslStream<S>) -> io::Result<Vec<u8>> {
    const MAX_FINISHED_SIZE: usize = 64;

    let mut finished = [0u8; MAX_FINISHED_SIZE];
    let finished_size = tls_stream.ssl().finished(&mut finished);
    let mut hasher = Sha512::new();
    hasher.update(&finished[..finished_size]);
    let finished_hash = hasher.finalize();

    let mut peer_finished = [0u8; MAX_FINISHED_SIZE];
    let peer_finished_size = tls_stream.ssl().peer_finished(&mut peer_finished);
    let mut hasher = Sha512::new();
    hasher.update(&peer_finished[..peer_finished_size]);
    let peer_finished_hash = hasher.finalize();

    let mut anded = [0u8; 64];
    for i in 0..64 {
        anded[i] = finished_hash[i] ^ peer_finished_hash[i];
    }

    let mut hasher = Sha512::new();
    hasher.update(anded);
    let hash = hasher.finalize()[..32].to_vec(); // the hash gets halved

    Ok(hash)
}

#[async_trait::async_trait]
impl Handshake for TlsNode {
    async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
        let node_conn_side = !conn.side();
        let stream = self.take_stream(&mut conn);
        let addr = conn.addr();

        let tls_stream = match node_conn_side {
            ConnectionSide::Initiator => {
                let ssl = self
                    .tls
                    .connector
                    .configure()
                    .unwrap()
                    .into_ssl("domain") // is SNI and hostname verification enabled?
                    .unwrap();
                let mut tls_stream = SslStream::new(ssl, stream).unwrap();

                Pin::new(&mut tls_stream).connect().await.map_err(|e| {
                    error!(parent: self.node().span(), "TLS handshake error: {}", e);
                    io::ErrorKind::InvalidData
                })?;

                // get the shared value based on the TLS handshake
                let shared_value = get_shared_value(&tls_stream)?;

                // prepare the crypto bits
                // let base58_pk = ToBase58::to_base58(&self.crypto.public_key.serialize()[..]);
                // FIXME: un-hardcode (it's more than just base58; this one was generated by rippled)
                let base58_pk = "n94MgqgdbMpSx5zMmffCqTcBUuKcazF62TPu1oio9zwxBAPXy3wQ";
                let sig = self.create_session_signature(&shared_value);

                // prepare the HTTP request message
                let mut request = Vec::new();
                request.extend_from_slice(b"GET / HTTP/1.1\r\n");
                request.extend_from_slice(b"User-Agent: rippled-1.9.1\r\n");
                request.extend_from_slice(b"Connection: Upgrade\r\n");
                request.extend_from_slice(b"Upgrade: XRPL/2.0, XRPL/2.1, XRPL/2.2\r\n"); // TODO: which ones should we handle?
                request.extend_from_slice(b"Connect-As: Peer\r\n");
                request.extend_from_slice(format!("Public-Key: {}\r\n", base58_pk).as_bytes());
                request.extend_from_slice(format!("Session-Signature: {}\r\n\r\n", sig).as_bytes());
                let request = Bytes::from(request);

                // use the HTTP codec to read/write the (post-TLS) handshake messages
                let codec = HttpCodec::new(self.node().span().clone(), HttpMsg::Response);
                let mut framed = Framed::new(&mut tls_stream, codec);

                trace!(parent: self.node().span(), "sending a request to {}: {:?}", addr, request);

                // send the handshake HTTP request message
                framed.send(request).await?;

                // read the HTTP request message (there should only be headers)
                let response_body = framed.try_next().await?.ok_or(io::ErrorKind::InvalidData)?;
                if !response_body.is_empty() {
                    warn!(parent: self.node().span(), "trailing bytes in the handshake response from {}: {:?}", addr, response_body);
                }

                tls_stream
            }
            ConnectionSide::Responder => {
                let ssl = Ssl::new(self.tls.acceptor.context()).unwrap();
                let mut tls_stream = SslStream::new(ssl, stream).unwrap();

                Pin::new(&mut tls_stream).accept().await.map_err(|e| {
                    error!(parent: self.node().span(), "TLS handshake error: {}", e);
                    io::ErrorKind::InvalidData
                })?;

                // get the shared value based on the TLS handshake
                let shared_value = get_shared_value(&tls_stream)?;

                // use the HTTP codec to read/write the (post-TLS) handshake messages
                let codec = HttpCodec::new(self.node().span().clone(), HttpMsg::Request);
                let mut framed = Framed::new(&mut tls_stream, codec);

                // read the HTTP request message (there should only be headers)
                let request_body = framed.try_next().await?.ok_or(io::ErrorKind::InvalidData)?;
                if !request_body.is_empty() {
                    warn!(parent: self.node().span(), "trailing bytes in the handshake request from {}: {:?}", addr, request_body);
                }

                // prepare the crypto bits
                // let base58_pk = ToBase58::to_base58(&self.crypto.public_key.serialize()[..]);
                // FIXME: un-hardcode (it's more than just base58; this one was generated by rippled)
                let base58_pk = "n94MgqgdbMpSx5zMmffCqTcBUuKcazF62TPu1oio9zwxBAPXy3wQ";
                let sig = self.create_session_signature(&shared_value);

                // prepare the response
                let mut response = Vec::new();
                response.extend_from_slice(b"HTTP/1.1 101 Switching Protocols\r\n");
                response.extend_from_slice(b"Connection: Upgrade\r\n");
                response.extend_from_slice(b"Upgrade: XRPL/2.2\r\n");
                response.extend_from_slice(b"Connect-As: Peer\r\n");
                response.extend_from_slice(b"Server: rippled-1.9.1\r\n");
                response.extend_from_slice(format!("Public-Key: {}\r\n", base58_pk).as_bytes());
                response
                    .extend_from_slice(format!("Session-Signature: {}\r\n\r\n", sig).as_bytes());
                let response = Bytes::from(response);

                trace!(parent: self.node().span(), "responding to {} with {:?}", addr, response);

                // send the handshake HTTP response message
                framed.send(response).await?;

                tls_stream
            }
        };

        self.return_stream(&mut conn, tls_stream);

        Ok(conn)
    }
}

#[async_trait::async_trait]
impl Reading for TlsNode {
    type Message = Message;
    type Codec = RippleCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        RippleCodec::new(self.node.span().clone())
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) -> io::Result<()> {
        info!(parent: self.node().span(), "read a message from {}: {:?}", source, message.payload);

        Ok(())
    }
}

impl Writing for TlsNode {
    type Message = Message;
    type Codec = RippleCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        RippleCodec::new(self.node.span().clone())
    }
}

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::INFO);

    let node_config = pea2pea::Config {
        listener_ip: Some("127.0.0.1".parse().unwrap()),
        desired_listening_port: Some(12345),
        allow_random_port: false,
        ..Default::default()
    };

    // generate the keypair and prepare the crypto engine

    let engine = Secp256k1::new();
    // let (private_key, public_key) = engine.generate_keypair(&mut secp256k1::rand::thread_rng());
    // FIXME: un-hardcode (taken from a fresh rippled node)
    let private_key = SecretKey::from_slice(
        &base64::decode(b"t/FfoDGZhMDfFiFHz42jYZfSuuaTU+Qrv9O5wAYEgaI=").unwrap(),
    )
    .unwrap();
    // FIXME: un-hardcode (taken from a fresh rippled node)
    let public_key = PublicKey::from_slice(
        &base64::decode(b"A/Y8pgzgBYwYYk70wLrwoDxtLoBYTEEDvEn0DWTms6QU").unwrap(),
    )
    .unwrap();
    let crypto = Arc::new(Crypto {
        engine,
        private_key,
        public_key,
    });

    // TLS acceptor

    let file = include_bytes!("/home/ljedrz/tmp/cert/identity.pfx");
    let identity = Pkcs12::from_der(&file[..]).unwrap().parse("pass").unwrap();
    let mut acceptor = SslAcceptor::mozilla_intermediate(SslMethod::tls()).unwrap();
    acceptor.set_private_key(&identity.pkey).unwrap();
    acceptor.set_certificate(&identity.cert).unwrap();
    let acceptor = acceptor.build();

    // TLS connector

    let mut connector = SslConnector::builder(SslMethod::tls()).unwrap();
    connector.set_verify(SslVerifyMode::NONE); // we might remove it once the keypair is solid
    let connector = connector.build();

    // the pea2pea node

    let tls_node = TlsNode {
        node: Node::new(Some(node_config)).await.unwrap(),
        crypto,
        tls: Tls {
            acceptor,
            connector,
        },
    };

    tls_node.enable_handshake().await;
    tls_node.enable_reading().await;
    tls_node.enable_writing().await;

    // uncomment to connect to a seed node
    tls_node
        .node()
        .connect("127.0.0.1:51235".parse().unwrap())
        .await
        .unwrap();

    std::future::pending::<()>().await;
}
