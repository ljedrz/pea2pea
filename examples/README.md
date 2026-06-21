# Examples

This directory contains a collection of runnable examples demonstrating how to use `pea2pea`. They are organized by complexity and use case.

To run any example, use the standard cargo command followed by the example name (e.g. `cargo run --example telephone_game`).

## 🎮 Fun & Visual (Tutorials)

These examples are the best place to start. They use gamified scenarios to demonstrate core concepts like network topologies, message passing, and basic state synchronization.
We recommend running them with `RUST_LOG=debug` (or even `trace`) to see the step-by-step flow of a P2P network.

| Example | Description | Key Features |
| :--- | :--- | :--- |
| **[Dining Philosophers](examples/dining_philosophers.rs)** | A complex concurrency problem mapped to P2P. Nodes must negotiate access to shared resources ("forks") using stateful request/response flows. | Ring Topology, Shared State, Deadlock Avoidance |
| **[Fixed Length Crusaders](examples/fixed_length_crusaders.rs)** | A *JoJo's Bizarre Adventure*-inspired node battle. Demonstrates custom handshakes where timing and sequence matter. | Handshake Logic, Timer/Sleep Logic, Custom Codec |
| **[Hapsburg's Plan B](examples/hapsburgs_plan_b.rs)** | A *Naked Gun* homage demonstrating how to trigger logic immediately upon disconnection. Nodes exchange "last words" before the connection drops. | OnDisconnect Protocol, Cleanup Logic |
| **[Hot Potato](examples/hot_potato_game.rs)** | Nodes pass a "hot potato" (message) around a random mesh. The potato count is tracked globally to verify delivery. | Mesh Topology, Random Routing, Atomic Counters |
| **[Telephone Game](examples/telephone_game.rs)** | A linear chain of nodes passing a string message from start to end, modifying it along the way. | Line Topology, Message Forwarding |

## 🛠️ Practical & Patterns

These examples demonstrate standard infrastructure patterns, security integrations, interop with other P2P libraries, and real-world node management.

| Example | Description | Key Features |
| :--- | :--- | :--- |
| **[Kademlia DHT](examples/kademlia_dht.rs)** | A minimal Kademlia DHT - the canonical P2P distributed hash table | DHT, All Protocols In Action |
| **[LAN Discovery](examples/lan_discovery.rs)** | A "zero-conf" example where nodes broadcast their presence via UDP beacons to automatically discover and connect to peers in the local network. | UDP Broadcasting, Automatic Discovery, Hybrid TCP/UDP |
| **[Libp2p Interop](examples/libp2p.rs)** | A fully compatible `libp2p` node that performs a Noise handshake and multiplexes streams using Yamux to talk to `rust-libp2p` nodes. | Interop, Noise Encryption, Yamux Multiplexing, Complex Handshake |
| **[Noise Handshake](examples/noise_handshake.rs)** | Implements a secure Noise_XX_25519 handshake using the `snow` library to encrypt all traffic between nodes. | Encryption, Stream Hijacking, Snow Library Integration |
| **[Rate Limiting](examples/rate_limiting.rs)** | A node that tracks peer statistics (messages/sec) and automatically disconnects peers that exceed a spam threshold. | Stats Tracking, Ban Logic, Traffic Analysis |
| **[Simple RPC](examples/simple_rpc.rs)** | Implements a request/response pattern over raw TCP using correlation IDs to map replies to callers. | Request/Response, Correlation IDs, Manual Protocol |
| **[TLS](examples/tls.rs)** | Wraps the underlying TCP stream in a TLS layer using `native-tls`, enabling secure, standard encrypted communication. | Encryption, Stream Wrapping, Native-TLS |

## 🧠 Stress Tests

These examples involve high-load stress testing. They demonstrate the upper limits of what the library can handle.

| Example | Description | Key Features |
| :--- | :--- | :--- |
| **[C10k](examples/c10k.rs)** | The classic *C10k problem*: a swarm of 10,000 persistent clients connect to a single node and hold their sockets open. Polls the live connection count until the node saturates (or times out at 60s). Stress-tests raw inbound connection capacity rather than any messaging protocol. | Connection Limits, Saturation, OS Tuning |
| **[C100k](examples/c100k.rs)** | Breaks the per-node `u16` connection ceiling by sharding: several independent nodes share one port via `SO_REUSEPORT`, with the kernel balancing 100,000 connections across them, all sized from one set of consts. Clients fan out over multiple loopback source IPs to clear the ephemeral-port ceiling too. | Listener Sharding, SO_REUSEPORT, OS Tuning |
| **[Connection Churn](examples/churn_stress.rs)** | A "thundering herd" simulation where clients rapidly connect, exchange data, and disconnect. Demonstrates low overhead in connection lifecycle management. | Stress Testing, High Churn, Performance |
| **[Dense Mesh](examples/dense_mesh.rs)** | Spawns a high density of nodes (default 200) and measures exact RAM usage per node and per connection. Demonstrates the library's tunable memory footprint. | Memory Profiling, High Density, Metrics |
