#![deny(missing_docs)]
#![deny(unsafe_code)]

//! **pea2pea** is a P2P library designed with the following use cases in mind:
//! - simple and quick creation of custom P2P networks
//! - testing/verifying network protocols
//! - benchmarking and stress-testing P2P nodes (or other network entities)
//! - substituting other, "heavier" nodes in local network tests

mod config;
mod known_peers;
mod node;
mod stats;
mod topology;

pub mod connections;
pub mod protocols;

pub use config::Config;
pub use connections::{Connection, ConnectionSide};
pub use known_peers::KnownPeers;
pub use node::Node;
pub use stats::Stats;
pub use topology::{connect_nodes, Topology};

/// A trait for objects containing a [`Node`]; it is required to implement protocols.
pub trait Pea2Pea {
    /// Returns a clonable reference to the node.
    fn node(&self) -> &Node;
}
