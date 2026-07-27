# pea2pea

[![Crates.io](https://img.shields.io/crates/v/pea2pea.svg)](https://crates.io/crates/pea2pea)
[![CI](https://github.com/ljedrz/pea2pea/actions/workflows/ci.yml/badge.svg)](https://github.com/ljedrz/pea2pea/actions/workflows/ci.yml)
[![chaos](https://github.com/ljedrz/pea2pea/actions/workflows/nightly-chaos.yml/badge.svg)](https://github.com/ljedrz/pea2pea/actions/workflows/nightly-chaos.yml)
[![Documentation](https://docs.rs/pea2pea/badge.svg)](https://docs.rs/pea2pea)
[![dependency status](https://deps.rs/repo/github/ljedrz/pea2pea/status.svg)](https://deps.rs/repo/github/ljedrz/pea2pea)

**A clean, modular, and lightweight peer-to-peer networking library for Rust.**

`pea2pea` abstracts away the complex, low-level boilerplate of P2P networking - TCP stream handling, connection pooling, framing, backpressure, etc. - allowing you to focus strictly on your network's logic and protocol implementation.

### 📖 Table of Contents

- [⚡ Why pea2pea?](#-why-pea2pea)
- [🚀 Quick Start](#-quick-start)
- [🧩 Architecture](#-architecture)
- [🔒 Security](#-security)
- [🌀 Chaos Testing](#-chaos-testing)
- [🏁 Benchmarking](#-benchmarking)
- [📚 Examples](#-examples)
- [🚧 Project Status](#-project-status)
- [🤝 Contributing](#-contributing)
- [📜 License](#-license)
- [🫛 Peapod](#-peapod)

---

### ⚡ Why pea2pea?

* **Battle-Tested in Production:** This library has been vendored and deployed in high-throughput, real-world decentralized networks, successfully managing complex topologies and heavy traffic.
* **Simplicity First:** No complex configuration objects or rigid frameworks. You can traverse and understand the codebase in a single afternoon.
* **Minimal Dependency Tree:** `pea2pea` only has **6** of the most scrutinized, native dependencies, which restricts supply chain attack surface, and grants lightning-fast compile times.
* **Uncompromising Performance:** Designed as a minimal abstraction layer, the library imposes negligible overhead, allowing your application to saturate the underlying network hardware or loopback interface limits.
* **Tiny Footprint:** The core node structure occupies just **~16kB of RAM**; per-connection memory usage starts at **~14kB** and scales directly with your configured buffer sizes.
* **Meticulously Tested:** A comprehensive collection of tests and examples ensures correctness, not to mention a host of punishing stress tests targeting heisenbugs; there is no `unsafe` code involved, and the concurrency contract is spelled out in [INVARIANTS.md](INVARIANTS.md).
* **Complete Control:** You dictate the application logic, and control **every** byte sent and received. Use slightly altered nodes to quickly set up chaos/fuzz/stress tests for your production nodes.

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

### 🧩 Architecture

`pea2pea` operates on a **modular "hooks" system**. You control the connection lifecycle by implementing specific traits, while the library handles the low-level async plumbing.

*(For a visual representation, see the **[Connection Lifecycle Graph](https://github.com/ljedrz/pea2pea/blob/master/assets/connection_lifetime.png)**)*

Simply implement the traits you need:
* **Handshake:** Secure your connections (TLS, noise, etc.), configure the stream, or exchange metadata.
* **Reading & Writing:** Define framing (codecs), message processing, and backpressure handling.
* **OnConnect & OnDisconnect:** Trigger logic when a connection is fully established or severed (cleanup, recovery).

For full details, refer to the **[protocols documentation](https://docs.rs/pea2pea/latest/pea2pea/protocols/index.html)**.

---

### 🔒 Security

`pea2pea` embraces security through simplicity. It mitigates common denial-of-service vectors by default:

* **Slowloris / Connection Exhaustion:** The configurable timeouts ensure that "creeper" connections that fail to handshake or send data are aggressively pruned, freeing up slots for legitimate peers.
* **SYN Floods / Rapid Churn:** The library's internal state machine handles high-frequency connect/disconnect events (churn) without leaking file descriptors or memory.
* **Malicious Payloads / Fuzzing:** The strict separation of the `Reading` protocol means that malformed packets or garbage data are rejected at the codec level, instantly dropping the offender before application logic is touched.
* **Resource Limits:** Hard caps on connection counts prevent bad actors from monopolizing your node's resources.

You could call the design philosophy "healthily paranoid." `pea2pea` treats the outside world - and even custom protocol layers - with systematic skepticism. Rather than assuming perfect execution, it wraps user-defined hooks and network events in rigid guardrails, isolating failures so that a single misbehaving peer or implementation oversight won't bring down the node.

For the security policy, see [SECURITY.md](SECURITY.md). For a catalog of the runtime
invariants the implementation upholds - each with its enforcement mechanism and failure
mode, along with the properties it deliberately does *not* promise - see
[INVARIANTS.md](INVARIANTS.md).

> **Challenge:** We invite you to try and break a `pea2pea`-powered node. Point your favorite stress-testing tool (like `hping3` or a custom fuzzer) at it; the node will hold its ground.

---

### 🌀 Chaos Testing

`pea2pea` is [routinely](https://github.com/ljedrz/pea2pea/actions/workflows/nightly-chaos.yml)
run through a randomized concurrency stress test built to surface synchronization bugs, leaks,
and lifecycle inconsistencies that scripted tests don't reach.

A pool of up to 32 live nodes is driven by 16 uncoordinated workers, each picking a random action
every iteration - node spawns and shutdowns, connects and disconnects, listener flapping,
broadcasts and unicasts - so every shared structure in the library stays contended. Four
mechanisms keep the run out of any steady state:

- **Swarm-sampled action mixes.** Every epoch the action weights are re-rolled
  from the run's seed - some actions dominate, others drop out entirely - so
  successive epochs explore different regimes (all-out churn, connection
  hoarding, drain-only) rather than one hand-tuned mix.
- **An adaptive governor.** A feedback loop measures executor lag and steers
  action pacing to keep the runtime contended but alive, so the test locates
  each host's saturation point instead of assuming one.
- **Burst storms.** Periodically, dozens of extra workers flood the pool at
  zero delay to push the executor into overload, then withdraw, so recovery is
  exercised as well.
- **Cancelled connections.** Some connection attempts are dropped part-way
  through setup - during the TCP connect, the handshake, the protocol wiring,
  or the `OnConnect` scheduling - so the rollback paths get exercised alongside
  the success paths. Each must either roll back cleanly or complete and stay
  consistent.

Watchdogs fail the run on the spot if an operation wedges past its time bounds, the workers stall
as a whole, a node exceeds its connection limits or keeps active connections past its shutdown, a
node's sent-byte count stops matching the messages it sent, or the file-descriptor and task counts
drift beyond their ceilings.

At the end of a run every counter must reconcile exactly: nodes spawned equals nodes shut down,
every `on_connect` is paired with an `on_disconnect`, every inbound frame decoded to a size some
peer actually sends, and nothing remains in flight. The properties being checked are catalogued in
[INVARIANTS.md](INVARIANTS.md).

The traffic is shaped to make the byte checks bite: each node sends one fixed payload size, drawn
from a set that straddles the framing layer's internal flush boundary - where batched writes are
most likely to be miscounted.

The test has repeatedly caught bugs in race windows narrow enough to need tens of millions of
operations to trigger once.

The chaos test is included in the repository as
[`tests/chaos.rs`](tests/tests/chaos.rs); it runs until interrupted (or for
`CHAOS_RUNTIME_SECS`), and the printed seed allows best-effort reproduction
of a given run's action sequences.

---

### 🏁 Benchmarking

`pea2pea` is designed to be as fast as the machine it runs on. To verify the throughput on your specific hardware, run the included benchmarks:

```bash
cargo bench -p benches
```

Be sure to also check out the stress tests included in the [examples](examples).

---

### 📚 Examples

Check out the [examples](examples) directory, which is organized by complexity and use case:

* **🎮 Fun & Visual (Tutorials):** Gamified scenarios like the **[Telephone Game](examples/examples/telephone_game.rs)** or **[Hot Potato](examples/examples/hot_potato_game.rs)** that demonstrate core concepts like topology, message passing, and basic state synchronization.
* **🛠️ Practical & Patterns:** Standard infrastructure patterns, including **[TLS](examples/examples/tls.rs)**, **[Noise Handshakes](examples/examples/noise_handshake.rs)**, and **[Rate Limiting](examples/examples/rate_limiting.rs)**.
* **🧠 Stress Tests:** High-load scenarios like **[C10k](examples/examples/c10k.rs)** or **[Dense Mesh](examples/examples/dense_mesh.rs)** that demonstrate the library's performance.

---

### 🚧 Project Status

**Current State: Stable & Feature-Complete.**

Despite the `0.x` versioning, `pea2pea` is considered **production-ready**. The core architecture is finished and proven.

* **API Stability:** The public API is stable. We do not anticipate breaking changes unless there's a **very** good reason to do so, and migration is trivial.
* **Scope:** The library is effectively in "maintenance mode" regarding features. Future development is strictly limited to **hardening internals** to ensure maximum reliability. We are not actively adding new features to the core.

---

### 🤝 Contributing

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for details on our strict scope policy.

---

### 📜 License

This project is dual-licensed under either:

* CC0 1.0 Universal ([LICENSE-CC0](LICENSE-CC0) or https://creativecommons.org/publicdomain/zero/1.0/)
* MIT License ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

---

### 🫛 Peapod

This library is part of the Peapod: a collection of small, composable Rust libraries for building robust peer-to-peer systems.

| Library | Purpose |
| ------- | ------- |
| `pea2pea` | Lightweight P2P networking primitive |
| `peashape` | Traffic shaping |
| `peaveil` | Privacy-oriented peer discovery |
| `peasub` | Metadata-private dissemination |
| `peaplex` | Optional stream multiplexing |
| `peaboard` | Reference application |

Each library does one thing well and composes naturally with the others.
