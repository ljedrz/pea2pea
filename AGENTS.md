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

# chaos smoke run (a good "definition of done" for library changes; ~2 min)
CHAOS_RUNTIME_SECS=115 cargo test -p tests --profile chaos chaos -- --ignored --nocapture
```

For non-trivial library changes, the full verification ladder is: fmt → clippy →
unit + integration tests → doc build → a chaos smoke run (above) on at least one
seed. The chaos test fails loudly (watchdogs + end-of-run invariants), so a green
run is meaningful evidence.

## `#![deny(unsafe_code)]` and `#![deny(missing_docs)]` — every public item must be documented.

## INVARIANTS.md is the concurrency contract
Before non-trivially changing `node.rs`, `connections.rs`, or `protocols/*`, read
[INVARIANTS.md](INVARIANTS.md): it catalogs every runtime invariant (lock ordering,
shutdown phases, cleanup layering, hook pairing) with its enforcement mechanism and
failure mode, plus the properties that deliberately do **not** hold. Any change to
shutdown/cleanup/handler semantics must keep that document in sync.

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
- Env vars: `CHAOS_SEED=<u64>` (repro), `CHAOS_FAST_TIMEOUTS` (short timeouts), `CHAOS_RUNTIME_SECS=<int>` (default: until interrupted), `CHAOS_SWARM=0` (pin the classic action mix instead of per-epoch swarm sampling), `CHAOS_EPOCH_SECS=<int>` (mix re-roll interval, default 30), `CHAOS_GOVERNOR=0` (pin the static delay range instead of adaptive pacing), `CHAOS_WATCHDOG=0` (disable the runtime watchdogs: action age, global progress, sampled connection limits, shutdown drain, fd/task ceilings), `CHAOS_BURST=0` (disable the periodic zero-delay worker storms).
- A watchdog violation or a failed end-of-run invariant is a **real finding**, not test flakiness — this test has repeatedly caught genuine library races (wedged connects, skipped hooks). Investigate before rerunning; the printed seed gives best-effort reproduction of the action mix sequence (interleaving remains scheduler-dependent).
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
- **Shutdown is two-phase** (`ShutdownState` in `node.rs`): a synchronous flag (checked inside lock-held sections) stops new work first; an async watch signal later makes the protocol handler tasks drain their queues and exit (setup handlers fail queued requests, hook handlers still run queued triggers — this preserves the `OnConnect`/`OnDisconnect` pairing). Aborting is only a timed fallback; see the wind-down invariant in INVARIANTS.md.
- **Protocol handler plumbing is shared** (`protocols/mod.rs`): `install_protocol_handler` (spawn/readiness/register/publish), `run_setup_handler_loop` (Handshake/Reading/Writing) and `run_hook_handler_loop` (OnConnect/OnDisconnect), plus `await_handler_response` (a bounded await guarding against messages stranded in closed handler channels). Changes to handler/drain semantics belong there, not in the five protocol files.
- Cleanup is RAII-guard-based throughout (`ConnectionGuard`, `DisconnectOnDrop`, `SenderCleanup`, `SchedulingGuard`, `NodeTaskAborter`, `DisconnectFinalizer`): every claimed resource has an owner whose `Drop` completes or rolls back on any exit path. Stale-guard protection is keyed by `Connection::id` (globally unique); do not introduce parallel identity schemes.

## Examples (`examples/`)
- Documentation-grade, user-facing code; clarity beats cleverness, and some repetition (the protocol-enable dance, `Config` construction) is deliberate pedagogy — don't extract it.
- Shared helpers live in `examples/src/`: `start_logger`, `SimpleCodec`, `PostcardCodec`, `await_connection` (use it instead of sleep-then-`connected_addrs()[0]`), plus the `noise`/`yamux` modules.
- **Gotcha**: examples do *not* use the `test` feature, so `max_connections_per_ip` defaults to `1` — any example where one loopback node handles several connections must set it explicitly in `Config`.
- Every terminating example must end with `shut_down().await` on its nodes (the lifecycle contract must be modeled, not just documented).
- Examples are **not executed in CI** — after touching them, run the quick terminating ones (`telephone_game`, `noise_handshake`, `tls`, `hapsburgs_plan_b`, `hot_potato_game`, `simple_rpc`, `rate_limiting`, `churn_stress`); `lan_discovery` and `hole_punching` run forever, `libp2p`/`dining_philosophers` take ~60s, `c10k`/`c100k`/`dense_mesh` are heavier.

## Conventions
- CHANGELOG.md: terse, lowercase bullets under `Added`/`Changed`/`Deprecated`/`Fixed`/`Removed`; mark rare/subtle items with "(edge case)" and breaking ones with "**breaking**". The top unreleased section's version must match `pea2pea/Cargo.toml` at release time.
- The `"shutting down"` error string is a cross-module contract — always construct it via `node::shutting_down_error()`.

## Known deferred items (deliberate, with reasons)
- Consolidating the shutdown flag + watch into a single `watch<ShutdownPhase>` — buys only representation aesthetics. Do not revisit without a functional motive.
- Reading-side idle timeout re-arms a timer per inbound frame; a coarse last-activity design would be cheaper but is a hot-path behavior change.
- Send-path failure logs (`queue_message`) are not rate-limited (unlike the reading side's drop-log suppression).
- Example-set gaps: no minimal hello-world example, and `OnConnect` has no teaching-tier example (it only appears in stress/capstone examples).
- Deterministic simulation (seed-faithful replay) was considered: the stochastic regime finds bugs fast enough, and re-runs are acceptable.

## Scope policy (CONTRIBUTING.md)
Micro-kernel philosophy. PRs accepted only for: bug fixes, perf improvements, docs, tests, or features impossible to implement in userland. New deps and protocol implementations are rejected.

## `profile.chaos`
```toml
[profile.chaos]
inherits = "release"
debug-assertions = true
overflow-checks = true
```
