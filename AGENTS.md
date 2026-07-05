# pea2pea — Agent Guide

## Workspace

Rust workspace (`resolver = "3"`, edition 2024, MSRV 1.88). Members:

| Crate | Path | Purpose |
|---|---|---|
| `pea2pea` | `pea2pea/` | Library (published) |
| `examples` | `examples/` | Standalone examples |
| `benches` | `benches/` | Benchmarks |
| `tests` | `tests/` | Integration tests |
| `test-utils` | `test-utils/` | Shared test helpers |

## Commands

```bash
# unit tests
cargo test -p pea2pea

# integration tests
cargo test -p tests

# single test by name
cargo test -p tests test_name

# chaos stress test (requires --ignored)
cargo test -p tests --profile chaos -- --ignored --nocapture

# benchmarks (divan)
cargo bench -p benches

# doc tests
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace

# lint / format (run in this order)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

## `#![deny(unsafe_code)]` and `#![deny(missing_docs)]` — every public item must be documented.

## Required: `Node::shut_down()`
The `Node` has a reference cycle — it will **not** be dropped automatically. Always call `shut_down().await` when done with a node. Do **not** call `shut_down` from inside a per-connection protocol hook (signal a separate task instead).

## `Config` — `test` feature changes defaults
When `pea2pea` is built with `features = ["test"]`:
- `listener_addr` defaults to `127.0.0.1:0` (instead of `0.0.0.0:0`)
- `max_connections_per_ip` defaults to `100` (instead of `1`)

Tests and benches use `features = ["test"]`, examples do not.

## Test infrastructure
- `test-utils` provides: `start_listening`, `wait_for_connections`, `wait_until`, `assert_consistent`, `start_default_nodes`, `FullNoopNode`, `BarrierNode`, and `WritingExt` (`.send_dm()` shorthand).
- Tests use a `TestNode` (newtype over `Node`) with `impl_messaging!` macro for `Reading`+`Writing` boilerplate.
- Topology tests use `connect_nodes(nodes, Topology::*)` and `BarrierNode`.

### Chaos test specifics
- Run: `cargo test -p tests --profile chaos chaos -- --ignored --nocapture`
- Env vars: `CHAOS_SEED=<u64>` (repro), `CHAOS_FAST_TIMEOUTS` (short timeouts), `CHAOS_RUNTIME_SECS=<int>` (default: until interrupted), `CHAOS_SWARM=0` (pin the classic action mix instead of per-epoch swarm sampling), `CHAOS_EPOCH_SECS=<int>` (mix re-roll interval, default 30), `CHAOS_GOVERNOR=0` (pin the static delay range instead of adaptive pacing), `CHAOS_WATCHDOG=0` (disable the runtime watchdogs: action age, global progress, sampled connection limits, shutdown drain, fd/task ceilings).
- Recommended sysctls for long runs:
  ```
  sudo sysctl -w net.ipv4.ip_local_port_range="1024 65535"
  sudo sysctl -w net.ipv4.tcp_max_tw_buckets=2000000
  sudo sysctl -w net.ipv4.tcp_tw_reuse=1
  sudo cpupower frequency-set -g performance
  ```

## Architecture
- **Protocols as traits**: implement `Pea2Pea` → `Handshake` | `Reading` | `Writing` | `OnConnect` | `OnDisconnect` on your wrapper struct.
- Enable protocols via `enable_handshake()`, `enable_reading()`, `enable_writing()`, `enable_on_connect()`, `enable_on_disconnect()`.
- Connection lifecycle: listen/connect → (handshake) → (reading + writing) → connected (on_connect) → disconnect → on_disconnect.
- Connections identified by `SocketAddr` (IP+port). Simultaneous bidirectional connects produce two distinct connections.
- Self-connect detection: best-effort (loopback + listening addr only). For full protection, implement tie-breaking in `Handshake`.

## Scope policy (CONTRIBUTING.md)
Micro-kernel philosophy. PRs accepted only for: bug fixes, perf improvements, docs, tests, or features impossible to implement in userland. New deps and protocol implementations are rejected.

## `profile.chaos`
```toml
[profile.chaos]
inherits = "release"
debug-assertions = true
overflow-checks = true
```
