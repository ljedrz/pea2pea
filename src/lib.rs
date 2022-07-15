#![deny(missing_docs)]
#![deny(unsafe_code)]

//! **pea2pea** is a simple, low-level, and customizable implementation of a TCP P2P node.

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
