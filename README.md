# pea2pea

[![Crates.io](https://img.shields.io/crates/v/pea2pea.svg)](https://crates.io/crates/pea2pea)
[![Documentation](https://docs.rs/pea2pea/badge.svg)](https://docs.rs/pea2pea)

**A clean, modular, and lightweight peer-to-peer networking library for Rust.**

`pea2pea` abstracts away the complex, low-level boilerplate of P2P networking - TCP stream handling, connection pooling, framing, backpressure, etc. - allowing you to focus strictly on your network's logic and protocol implementation.

---

### ⚡ Why pea2pea?

* **Battle-Tested in Production:** This library has been vendored and deployed in high-throughput, real-world decentralized networks, successfully managing complex topologies and heavy traffic loads in production environments.
* **Simplicity First:** No complex configuration objects, massive dependency trees, or rigid frameworks.
* **Async by Default:** Built on top of `tokio`, fully non-blocking and performant.
* **Fast and Lightweight:** The potential throughput is over 40GB/s (tested locally on a single Ryzen 9 9950X), and a single node occupies from ~20kB to ~150kB of RAM. You can run **thousands** of them locally.
* **Meticulously Tested:** A comprehensive collection of tests and examples ensures correctness; there is no `unsafe` code involved.
* **Complete Control:** You dictate the application logic, and control **every** byte sent and received. Use slightly altered nodes to fuzz-test and stress-test your production nodes.

---

### 🚧 Project Status & Stability

**Current State: Stable & Feature-Complete.**

Despite the `0.x` versioning, `pea2pea` is considered **production-ready**. The core architecture is finished and proven.

* **API Stability:** The public API is stable. We do not anticipate breaking changes.
* **Scope:** The library is effectively in "maintenance mode" regarding features. Future development is strictly limited to **hardening internals** to ensure maximum reliability. We are not adding new features to the core.

---

### 🚀 Quick Start

Spin up a TCP node capable of receiving messages in 36 lines of code:

```rust
use std::{io, net::SocketAddr};

use pea2pea::{Config, ConnectionSide, Node, Pea2Pea, protocols::Reading};

// Define your node
#[derive(Clone)]
struct MyNode {
    p2p: Node,
    // add your state here
}

// Implement the Pea2Pea trait
impl Pea2Pea for MyNode {
    fn node(&self) -> &Node {
        &self.p2p
    }
}

// Specify how to read network messages
impl Reading for MyNode {
    type Message = bytes::BytesMut;
    type Codec = tokio_util::codec::LengthDelimitedCodec;

    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    async fn process_message(&self, source: SocketAddr, _message: Self::Message) {
        tracing::info!(parent: self.node().span(), "received a message from {source}");
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    // Log events
    tracing_subscriber::fmt::init();

    // Create the node's configuration
    let config = Config {
        listener_addr: Some("127.0.0.1:0".parse().unwrap()),
        ..Default::default()
    };

    // Instantiate the node
    let node = MyNode {
        p2p: Node::new(config),
    };

    // Start reading incoming messages according to the Reading protocol
    node.enable_reading().await;

    // Start accepting connections
    node.p2p.toggle_listener().await?;

    // Keep the node running
    std::future::pending::<()>().await;

    Ok(())
}
```

---

### ⚙️ Architecture & Customization

`pea2pea` is designed around a "hooks" system. You implement specific traits (or enable built-in capabilities) to control the entire lifecycle of a connection.

*(For a visual representation of the connection state machine, please refer to the **Connection Lifecycle Graph** from the [documentation](https://docs.rs/pea2pea/latest/pea2pea/protocols/index.html).*

You have full control over every stage:

#### 1. Connection Logic
* **Filtering:** Reject unwanted incoming connections (e.g., IP bans, max peer counts) before they consume resources.
* **Creation:** Define exactly when and how to establish new outbound connections.

#### 2. Handshaking (Security)
* **Authentication:** Exchange headers, keys, or magic bytes immediately after connecting.
* **Encryption:** Wrap streams (e.g., Noise, TLS) before application data flows.
* **Validation:** Drop connections immediately if the handshake fails.

#### 3. Communication (Read/Write)
* **Framing:** Use one of the [codecs](https://docs.rs/tokio-util/latest/tokio_util/codec/index.html#structs) from `tokio_util` or provide your own.
* **Protocol:** Handle incoming messages and route them to your application logic.
* **Backpressure:** The library manages socket pressure, ensuring your node doesn't get overwhelmed.

#### 4. Disconnection Logic
* **Cleanup:** Hook into disconnect events to clean up peer state.
* **Recovery:** Decide whether to attempt a re-connection or ban the peer based on the disconnect reason.

---

### 📦 Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
pea2pea = "x.x.x" # Replace with the latest version
tokio = { version = "1", features = ["rt"] } # pick any other features you need
```

### 📚 Examples

Check out the [examples](https://github.com/ljedrz/pea2pea/tree/master/examples) directory for real-world usage patterns, including:
* **Fixed Topology:** Creating a static mesh of nodes.
* **Gossip:** Implementing a basic gossip protocol.
* **Secure Handshake:** How to implement authentication headers.
* **`libp2p` Interop:** Connect to a simple `libp2p` node.

### 🤝 Contributing

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for details on our strict scope policy.
