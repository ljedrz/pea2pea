# Examples

This directory contains a collection of runnable examples demonstrating how to use `pea2pea`. They are organized by complexity and use case.

To run any example, use the standard cargo command followed by the example name (e.g. `cargo run --example telephone_game`).

## 🎮 Fun & Visual (Tutorials)

These examples are the best place to start. They use gamified scenarios to demonstrate core concepts like network topologies, message passing, and basic state synchronization.

| Example | Description | Key Features |
| :--- | :--- | :--- |
| **[Fixed Length Crusaders](fixed_length_crusaders.rs)** | A *JoJo's Bizarre Adventure*-inspired node battle. Demonstrates custom handshakes where timing and sequence matter. | Handshake Logic, Timer/Sleep Logic, Custom Codec |
| **[Hapsburg's Plan B](hapsburgs_plan_b.rs)** | A *Naked Gun* homage demonstrating how to trigger logic immediately upon disconnection. Nodes exchange "last words" before the connection drops. | OnDisconnect Protocol, Cleanup Logic |
| **[Hot Potato](hot_potato_game.rs)** | Nodes pass a "hot potato" (message) around a random mesh. The potato count is tracked globally to verify delivery. | Mesh Topology, Random Routing, Atomic Counters |
| **[Telephone Game](telephone_game.rs)** | A linear chain of nodes passing a string message from start to end, modifying it along the way. | Line Topology, Message Forwarding |

## 🛠️ Practical & Patterns

These examples demonstrate standard infrastructure patterns, security integrations, and real-world node management.

| Example | Description | Key Features |
| :--- | :--- | :--- |
| **[LAN Discovery](lan_discovery.rs)** | A "zero-conf" example where nodes broadcast their presence via UDP beacons to automatically discover and connect to peers in the local network. | UDP Broadcasting, Automatic Discovery, Hybrid TCP/UDP |
| **[Noise Handshake](noise_handshake.rs)** | Implements a secure Noise_XX_25519 handshake using the `snow` library to encrypt all traffic between nodes. | Encryption, Stream Hijacking, Snow Library Integration |
| **[Rate Limiting](rate_limiting.rs)** | A node that tracks peer statistics (messages/sec) and automatically disconnects peers that exceed a spam threshold. | Stats Tracking, Ban Logic, Traffic Analysis |
| **[Simple RPC](simple_rpc.rs)** | Implements a request/response pattern over raw TCP using correlation IDs to map replies to callers. | Request/Response, Correlation IDs, Manual Protocol |
| **[TLS](tls.rs)** | Wraps the underlying TCP stream in a TLS layer using `native-tls`, enabling secure, standard encrypted communication. | Encryption, Stream Wrapping, Native-TLS |

## 🧠 Advanced & Stress Tests

These examples involve complex state machines, high-load stress testing, or heavy interoperability. They demonstrate the upper limits of what the library can handle.

| Example | Description | Key Features |
| :--- | :--- | :--- |
| **[Bucket Brigade](bucket_brigade.rs)** | A 100-node linear chain that passes a message from start to end. Measures per-hop latency, demonstrating the minimal overhead of the protocol stack. | Latency Testing, Topology, Forwarding |
| **[Connection Churn](churn_stress.rs)** | A "thundering herd" simulation where clients rapidly connect, exchange data, and disconnect. Demonstrates low overhead in connection lifecycle management. | Stress Testing, High Churn, Performance |
| **[Dense Mesh](dense_mesh.rs)** | Spawns a high density of nodes (default 25) and measures exact RAM usage per node and per connection using the `peak_alloc` allocator. Demonstrates the library's tunable memory footprint. | Memory Profiling, High Density, Metrics |
| **[Dining Philosophers](dining_philosophers.rs)** | A complex concurrency problem mapped to P2P. Nodes must negotiate access to shared resources ("forks") using stateful request/response flows. | Ring Topology, Shared State, Deadlock Avoidance |
| **[Libp2p Interop](libp2p.rs)** | A fully compatible `libp2p` node that performs a Noise handshake and multiplexes streams using Yamux to talk to `rust-libp2p` nodes. | Interop, Noise Encryption, Yamux Multiplexing, Complex Handshake |
| **[Packet Cannon](packet_cannon.rs)** | A raw throughput benchmark measuring Packets Per Second (PPS). Uses zero-allocation codecs to isolate the library's internal overhead and measure the upper limits of message frequency. | PPS Benchmarking, Zero-Copy Codecs, Backpressure |

## 🧩 Common Utilities

* **[common/](common/)**: Shared utilities for the examples, including a simple length-delimited codec (`TestCodec`) and logger setup.
