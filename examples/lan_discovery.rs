//! A "Magic" LAN Discovery example.
//!
//! Nodes broadcast their presence via UDP beacons.
//! When a node detects a beacon from a peer, it automatically establishes
//! a TCP connection using `pea2pea`.
//!
//! Run this example in two or more different terminal windows or computers
//! in your local network to see them find each other!

mod common;

use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
    ops::RangeInclusive,
    time::Duration,
};

use bytes::{Buf, BufMut, Bytes};
use pea2pea::{
    Config, ConnectionSide, Node, Pea2Pea,
    protocols::{Reading, Writing},
};
use tokio::net::UdpSocket;
use tokio::time::sleep;
use tracing::*;
use tracing_subscriber::filter::LevelFilter;

// scan a range of ports to avoid "address already in use"
// when running multiple nodes on the same machine.
const DISCOVERY_PORT_RANGE: RangeInclusive<u16> = 8590..=8599;

// a magic header to filter out stray packets on the network.
const MAGIC_BYTES: &[u8] = b"PEA2PEA_DISCO";

#[derive(Clone)]
struct DiscoveryNode(Node);

impl Pea2Pea for DiscoveryNode {
    fn node(&self) -> &Node {
        &self.0
    }
}

impl Reading for DiscoveryNode {
    type Message = String;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, source: SocketAddr, message: Self::Message) {
        info!(parent: self.node().span(), "📩 received from {source}: \"{message}\"");
    }
}

impl Writing for DiscoveryNode {
    type Message = String;
    type Codec = common::TestCodec<Self::Message>;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

impl DiscoveryNode {
    /// start the UDP discovery service
    /// 1. bind to the first available port in the DISCOVERY_PORT_RANGE
    /// 2. broadcasts our TCP port to all ports in that range every second
    /// 3. listen for broadcasts and connect
    async fn start_discovery(&self, tcp_port: u16) -> io::Result<()> {
        let mut listener = None;
        let mut my_discovery_port = 0;

        // try to bind to a port in the discovery range
        for port in DISCOVERY_PORT_RANGE {
            if let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).await {
                socket.set_broadcast(true)?;
                info!(parent: self.node().span(), "discovery service active on UDP port {port}");
                my_discovery_port = port;
                listener = Some(socket);
                break;
            }
        }

        let listener =
            listener.expect("failed to bind to any discovery port; are 10 nodes already running?");

        // create a separate socket for sending (ephemeral source port)
        let sender = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
        sender.set_broadcast(true)?;

        // --- task 1: the beacon (sender) ---
        tokio::spawn(async move {
            let mut buf = Vec::new();
            loop {
                buf.clear();
                buf.put_slice(MAGIC_BYTES);
                buf.put_u16(tcp_port);

                // we broadcast to every port in the range to ensure we hit
                // whatever port the other nodes happened to bind to
                for target_port in DISCOVERY_PORT_RANGE {
                    // skip sending to ourselves if we are running on localhost
                    if target_port == my_discovery_port {
                        continue;
                    }

                    let target = SocketAddr::from(([255, 255, 255, 255], target_port));
                    if let Err(e) = sender.send_to(&buf, target).await {
                        // ignore send errors (network might be unreachable, etc)
                        trace!("failed to broadcast to {target_port}: {e}");
                    }
                }

                sleep(Duration::from_secs(1)).await;
            }
        });

        // --- task 2: the listener (receiver) ---
        let node = self.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            loop {
                match listener.recv_from(&mut buf).await {
                    Ok((len, remote_addr)) => {
                        // validate packet size
                        if len < MAGIC_BYTES.len() + 2 {
                            continue;
                        }

                        // validate magic bytes
                        let mut data = Bytes::copy_from_slice(&buf[..len]);
                        let magic = data.split_to(MAGIC_BYTES.len());
                        if magic != MAGIC_BYTES {
                            continue;
                        }

                        // extract the peer's TCP port
                        let peer_tcp_port = data.get_u16();
                        let peer_tcp_addr = SocketAddr::new(remote_addr.ip(), peer_tcp_port);

                        // don't connect to ourselves (TCP check)
                        if peer_tcp_port == tcp_port {
                            continue;
                        }

                        // connect!
                        if !node.node().is_connected(peer_tcp_addr)
                            && !node.node().is_connecting(peer_tcp_addr)
                        {
                            info!(parent: node.node().span(), "✨ discovered peer at {peer_tcp_addr}!");

                            match node.node().connect(peer_tcp_addr).await {
                                Ok(_) => {
                                    let _ = node.unicast(peer_tcp_addr, "I see you!".into());
                                }
                                Err(e) => {
                                    error!("Couldn't connect to {peer_tcp_addr}: {e}");
                                }
                            }
                        }
                    }
                    Err(e) => error!("UDP recv error: {e}"),
                }
            }
        });

        Ok(())
    }
}

#[tokio::main]
async fn main() {
    common::start_logger(LevelFilter::INFO);

    let config = Config {
        name: Some("explorer".into()),
        listener_addr: Some((Ipv4Addr::UNSPECIFIED, 0).into()),
        ..Default::default()
    };
    let node = DiscoveryNode(Node::new(config));

    node.enable_reading().await;
    node.enable_writing().await;

    let tcp_addr = node.node().toggle_listener().await.unwrap().unwrap();
    info!(parent: node.node().span(), "listening on TCP {tcp_addr}");

    if let Err(e) = node.start_discovery(tcp_addr.port()).await {
        error!("failed to start discovery: {e}");
    }

    std::future::pending::<()>().await;
}
