#![deny(missing_docs)]

//! **pea2pea** is a P2P library designed with the following use cases in mind:
//! - simple and quick creation of custom P2P networks
//! - testing/verifying network protocols
//! - benchmarking and stress-testing P2P nodes (or other network entities)
//! - substituting other, "heavier" nodes in local network tests

mod config;
mod connection;
mod connections;
mod known_peers;
mod node;
mod node_stats;
mod protocols;
mod topology;

pub use config::NodeConfig;
pub use connection::{Connection, ConnectionReader, ConnectionSide};
pub use known_peers::{KnownPeers, PeerStats};
pub use node::Node;
pub use node_stats::NodeStats;
pub use protocols::{HandshakeHandler, HandshakeObjects, Handshaking, InboundHandler, Messaging};
pub use topology::{connect_nodes, Topology};

/// A trait for objects containing a `Node`; it is required to implement protocols.
pub trait Pea2Pea {
    /// Returns a clonable reference to the node.
    fn node(&self) -> &std::sync::Arc<Node>;
}
