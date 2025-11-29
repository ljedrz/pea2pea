# pea2pea

[![Crates.io](https://img.shields.io/crates/v/pea2pea.svg)](https://crates.io/crates/pea2pea)
[![Documentation](https://docs.rs/pea2pea/badge.svg)](https://docs.rs/pea2pea)

**A clean, modular, and lightweight peer-to-peer networking library for Rust.**

`pea2pea` abstracts away the complex, low-level boilerplate of P2P networking - TCP stream handling, connection pooling, framing, backpressure, etc. - allowing you to focus strictly on your network's logic and protocol implementation.

---

### ⚡ Why pea2pea?

* **Battle-Tested in Production:** This library has been vendored and deployed in high-throughput, real-world decentralized networks, successfully managing complex topologies and heavy traffic.
* **Simplicity First:** No complex configuration objects or rigid frameworks. You can audit the library yourself in a single afternoon.
* **Minimal Dependency Tree:** `pea2pea` relies strictly on `tokio` and standard crates, resulting in lightning-fast compile times and tiny binaries.
* **Uncompromising Performance:** Designed as a zero-weight abstraction layer, the library imposes negligible overhead, allowing your application to saturate the underlying network hardware or loopback interface limits.
* **Tiny Footprint:** The core node structure occupies just **~16kB of RAM**; per-connection memory usage starts at **~14kB** and scales directly with your configured buffer sizes.
* **Meticulously Tested:** A comprehensive collection of tests and examples ensures correctness; there is no `unsafe` code involved.
* **Complete Control:** You dictate the application logic, and control **every** byte sent and received. Use slightly altered nodes to fuzz-test and stress-test your production nodes.

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

`pea2pea` operates on a **modular "hooks" system**. You control the connection lifecycle by implementing specific traits, while the library handles the low-level async plumbing.

*(For a visual representation, see the **[Connection Lifecycle Graph](https://github.com/ljedrz/pea2pea/blob/master/assets/connection_lifetime.png)**)*

Simply implement the traits you need:
* **Handshake:** Secure your connections (tls, noise, etc.), configure the stream, or exchange metadata.
* **Reading & Writing:** Define framing (codecs), message processing, and backpressure handling.
* **OnConnect / OnDisconnect:** Trigger logic when a connection is fully established or severed (cleanup, recovery).

For full details, refer to the **[protocols documentation](https://docs.rs/pea2pea/latest/pea2pea/protocols/index.html)**.

---

### 🛡️ Security & Resilience

`pea2pea` is built to survive the hostile internet. Its architecture naturally mitigates common denial-of-service vectors without requiring complex configuration:

* **Slowloris / Connection Exhaustion:** The `connection_timeout_ms` config ensures that "creeper" connections that fail to handshake or send data are aggressively pruned, freeing up slots for legitimate peers.
* **SYN Floods / Rapid Churn:** The library's internal state machine handles high-frequency connect/disconnect events (churn) without leaking file descriptors or memory.
* **Malicious Payloads / Fuzzing:** The strict separation of the `Reading` protocol means that malformed packets or garbage data are rejected at the codec level, instantly dropping the offender before application logic is touched.
* **Resource Limits:** Hard caps on `max_connections` and `max_connections_per_ip` prevent bad actors from monopolizing your node's resources.

> **Challenge:** We invite you to try and break a `pea2pea`-powered node. Point your favorite stress-testing tool (like `hping3` or a custom fuzzer) at it; the node will hold its ground.

---

### Benchmarking

`pea2pea` is designed to be as fast as the machine it runs on. To verify the throughput on your specific hardware, run the included benchmark suite:

```rust
cargo test --release --test benches -- --nocapture --ignored
```

Be sure to also check out the stress tests included in the [examples](examples).

---

### 📚 Examples

Check out the [examples](examples) directory, which is organized by complexity and use case:

* **🎮 Fun & Visual (Tutorials):** Gamified scenarios like the **[Telephone Game](examples/telephone_game.rs)** or **[Hot Potato](examples/hot_potato_game.rs)** that demonstrate core concepts like topology, message passing, and basic state synchronization.
* **🛠️ Practical & Patterns:** Standard infrastructure patterns, including **[TLS](examples/tls.rs)**, **[Noise Handshakes](examples/noise_handshake.rs)**, **[Rate Limiting](examples/rate_limiting.rs)**, and **[RPC](examples/simple_rpc.rs)**.
* **🧠 Advanced & Stress Tests:** High-load scenarios like **[Connection Churn](examples/churn_stress.rs)** or **[Dense Mesh](examples/dense_mesh.rs)** that demonstrate the library's performance and **[libp2p interop](examples/libp2p.rs)**.

---

### 📦 Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
pea2pea = "x.x.x" # replace with the latest version
tokio = { version = "1", features = ["rt"] } # pick any other features you need
```

---

### 🚧 Project Status & Stability

**Current State: Stable & Feature-Complete.**

Despite the `0.x` versioning, `pea2pea` is considered **production-ready**. The core architecture is finished and proven.

* **API Stability:** The public API is stable. We do not anticipate breaking changes unless there's a **very** good reason to do so.
* **Scope:** The library is effectively in "maintenance mode" regarding features. Future development is strictly limited to **hardening internals** to ensure maximum reliability. We are not adding new features to the core.

---

### 🤝 Contributing

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for details on our strict scope policy.
