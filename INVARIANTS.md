# Invariants

This document enumerates the properties that the `pea2pea` implementation
maintains at runtime. Each entry names an invariant, identifies the code
that enforces it, and notes what would break if the invariant failed. The
target audience is auditors, contributors making non-trivial changes, and
anyone reasoning about whether a proposed modification preserves the
library's correctness contract.

An invariant in this document is a property that holds at all times during
normal operation, enforced by the implementation. Violating one is a bug,
not a design choice. The file is organized by property, not by module - a
single invariant may span several files, and is stated once.

The closing section ("Non-invariants") lists properties that look like
they *should* hold but deliberately do not, to prevent over-reading the
contract.

---

## Lock ordering: `limits → active`

**Statement.** Any code path that requires both `Connections::limits`
(`parking_lot::Mutex<ConnectionLimits>`) and `Connections::active`
(`parking_lot::RwLock<HashMap<SocketAddr, Connection>>`) must acquire
`limits` before `active`. The reverse order is forbidden.

**Enforcement.** `Node::check_and_reserve` (`src/node.rs`) acquires
`limits`, then takes a single `active` read guard that serves both the
duplicate check and the connection count. `DisconnectFinalizer::drop`
(`src/node.rs`) follows the same order: `limits` first, then the write
to `active` inside `Connections::remove`. `Connections::add`
(`src/connections.rs`) enforces the complementary discipline: while it
holds the `active` write lock, the `ConnectionGuard` (whose `Drop` locks
`limits`) must not be dropped - the guard is released only after the
`active` guard goes out of scope. Inline comments in all three locations
call out the ordering explicitly to prevent future inversions.

**Failure mode.** A reverse-order acquisition (`active` write held while
waiting on `limits`) creates a classical AB-BA deadlock with any
concurrent `check_and_reserve` invocation. Whole-node hang, no recovery.

---

## Shutdown is monotonic and serialized with `active.add`

**Statement.** Once `Node::shut_down` marks the shutdown as underway
(the flag inside `InnerNode.shutdown`, a `ShutdownState`), no further
`Connection` will ever be inserted into `Connections::active`. The flag
is monotonic (set once, never cleared).

**Enforcement.** `Node::shut_down` (`src/node.rs`) calls
`self.shutdown.begin()` - a `Release` store of the flag - as its very
first action. `Connections::add` (`src/connections.rs`) acquires the
`active` write lock, then checks `shutdown.is_underway()` (an `Acquire`
load); if set, the insert is refused with a "shutting down" error and
the `Connection` is dropped on the spot, its `Drop` aborting the
connection's tasks. Because the flag read happens *inside* the
write-lock critical section, and `shut_down`'s subsequent snapshot via
`connected_addrs()` takes the same `active` read lock, the two
operations cannot interleave so as to leak a connection past the
snapshot.

**Failure mode.** If the flag read moved outside the lock, a racing
`adapt_stream` could insert a connection after `shut_down`'s snapshot;
with no owner committed to removing that entry, the active-set drain in
`shut_down` would hang forever.

---

## Disconnect is idempotent and exclusive per connection

**Statement.** At most one call to `Node::disconnect(addr)` for any given
connection will execute the cleanup body. All subsequent calls
(regardless of source) return `false` immediately without side effects.

**Enforcement.** `Node::disconnect_w_origin` (`src/node.rs`) - which
every disconnect path, including the public `Node::disconnect`, routes
through - reads the connection under `active.read()`, then calls
`conn.disconnecting.swap(true, AcqRel)`. The first caller observes
`false → true` and proceeds; all others observe `true → true` and
return. This gate is what permits multiple cleanup pathways - explicit
user `disconnect()`, `DisconnectOnDrop` from reader/writer task abort,
the concurrent per-peer disconnect fan-out in `shut_down` (a
`FuturesUnordered` of `disconnect_w_origin` futures) - to coexist
without coordination.

**Failure mode.** Without the swap, concurrent disconnects could
double-fire `OnDisconnect`, double-remove from the senders map, or
double-decrement IP counts. The latter would either trip the
double-release `debug_assert` in `ConnectionLimits::release_ip` or
wrongly free a per-IP slot in a still-live bucket, breaking subsequent
connection accounting for the same IP.

---

## Connection cleanup is layered, with each layer self-sufficient

**Statement.** A `Connection` reliably tears down regardless of which
code path initiated the teardown. The cleanup layers are, in order of
proximity to the connection:

1. `Connection::Drop` aborts every `JoinHandle` in `conn.tasks` in
   reverse-push order.
2. `SenderCleanup::Drop` (held by the writer task) removes the
   `(addr → sender)` entry from the senders map, guarded by the connection id
   (see next section).
3. `DisconnectOnDrop::Drop` (held by the reader, writer,
   inbound-message, and `OnConnect` tasks - the last defused on a
   successful `on_connect`) spawns a fire-and-forget, connection-id-scoped
   disconnect (`disconnect_w_origin` with the guard's `conn_id`), which
   the exclusivity gate above renders idempotent and the id check renders
   harmless to any newer connection reusing the address.
4. `Node::shut_down` fans out per-peer disconnects concurrently and awaits
   them all, then signals the protocol handler tasks to wind down gracefully
   (see the handler wind-down invariant below), aborting only what remains.

Any single layer firing in isolation produces a clean teardown; layers
firing in combination remain correct because each is idempotent.

**Enforcement.** Located in `src/connections.rs` (`Connection::Drop`),
`src/protocols/writing.rs` (`SenderCleanup`), `src/protocols/mod.rs`
(`DisconnectOnDrop`), and `Node::shut_down` (`src/node.rs`).

**Failure mode.** Removing any layer leaves a class of failure paths
uncleaned - e.g., removing `DisconnectOnDrop` means a panicking user
`process_message` would leave the connection in `active` indefinitely:
`Connection::Drop` (the task aborter) only runs once the connection is
removed from `active`, which would then never happen, and neither the
senders map nor the IP counts would be cleaned up.

---

## Senders map removal is connection-id-gated

**Statement.** `SenderCleanup::Drop` removes the `(addr → sender)` entry
from `WritingHandler::senders` only if the entry's connection id matches
its own. If a newer connection has reused `addr` and overwritten the
entry, the cleanup is a no-op.

**Enforcement.** `SenderCleanup::Drop` in `src/protocols/writing.rs`
checks `e.get().0 == self.conn_id` before calling `e.remove()`. The tag
is the connection's unique instance id (`Connection::id`, allocated by a
global monotonic counter) - the same identity that `DisconnectOnDrop`
and the OnConnect plumbing use to recognize address reuse - so every
connection's sender entry carries a unique tag.

**Failure mode.** Without the connection-id check, a connection that
reconnects after a disconnect (same `addr`) would race: the old
connection's `SenderCleanup::Drop` could fire after the new connection
had installed its sender, removing the new entry. Subsequent `unicast`
calls would see `NotConnected` despite the new connection being live.

Note that the disconnect path's direct senders-map removal (in
`DisconnectFinalizer::drop`, `src/node.rs`) is *not* connection-id-gated.
This is sound only because `addr` reuse during the disconnect's
critical section is structurally impossible given the surrounding
exclusivity invariants. If those invariants were ever weakened (e.g., by
reintroducing concurrent same-addr connections), this removal would also
need gating.

---

## OnConnect / OnDisconnect pairing is bounded and predictable

**Statement.** For any given connection that successfully entered
`active`:

- **With `OnConnect::ABORTABLE = true` (default):** `on_connect` is
  best-effort. Under rare contention, the spawned `on_connect` task may
  be aborted before its first poll if a disconnect races in, in which
  case `on_connect` never runs. `on_disconnect` still fires.
- **With `OnConnect::ABORTABLE = false`:** `on_connect` is guaranteed to
  run to completion. The task is detached from `conn.tasks`, so neither
  `disconnect` nor `shut_down` can abort it. During node shutdown the
  guarantee is preserved by the graceful wind-down invariant below; only
  its documented hard-abort escape hatches can bypass it.

In either configuration, `on_disconnect` fires exactly once per
connection-that-reached-`active`, courtesy of the exclusivity gate.

**Enforcement.** The detached scheduling task spawned by
`Node::adapt_stream` (`src/node.rs`) branches on the `ABORTABLE` flag
returned from the OnConnect handler: it either attaches the hook's
handle to `conn.tasks` (abortable case; the attach is gated on the
connection's instance id, so an address reused by a newer connection is
never handed a stranger's task, and the hook is aborted instead) or
drops the handle (detached case).

**Observable property.** Across long runs, the differential between
`on_connect`-fired and `on_disconnect`-fired counters is bounded above by
the number of in-flight connections at the observation moment, plus the
small race-induced asymmetry under the default `ABORTABLE = true`. With
`ABORTABLE = false`, the differential is bounded only by the in-flight
count and converges to exact equality once all connections close.

**Failure mode.** Loss of the `ABORTABLE = false` guarantee (e.g., by
accidentally pushing the handle to `conn.tasks`) would make `on_connect`
silently droppable under load, breaking any application that uses it for
bookkeeping that must pair with `on_disconnect`.

---

## Connection registration precedes the OnConnect trigger

**Statement.** The user's `on_connect` callback is invoked only after the
connection has been added to `active` and is fully operational (reader
and writer tasks running, senders entry installed, readiness signal
sent). At the moment `on_connect` runs, the user may successfully call
`disconnect`, `unicast`, or any other connection-aware API for the
peer.

**Enforcement.** The order in `Node::adapt_stream` (`src/node.rs`) is
fixed: `enable_protocols` (handshake, reader, writer setup) → take
`readiness_notifier` → `connections.add` (which consumes the
`ConnectionGuard`, finalizing the reservation) → send the readiness
signal → trigger the OnConnect handler (via a detached scheduling
task).

**Failure mode.** Reordering OnConnect to run before `connections.add`
would break any user code that calls `node.disconnect(peer_addr)` from
within `on_connect`, since the addr wouldn't be in `active` to find.
Likewise, calling OnConnect before the readiness signal would deadlock
any `on_connect` that awaits an inbound message from the peer.

---

## Per-IP and per-node connection counts are atomically reserved

**Statement.** At any moment, the value of `limits.ip_counts.get(ip)`
equals the actual number of established-or-establishing connections
involving that IP (the `limits.connecting` set tracks the establishing
subset, by address). The `ConnectionGuard` RAII type guarantees this
even on early-return failure paths in `adapt_stream`.

**Enforcement.** `Node::check_and_reserve` (`src/node.rs`) increments
`ip_counts` and inserts into `connecting` atomically under
`limits.lock()`. The returned `ConnectionGuard` always clears the
`connecting` entry on drop, and additionally releases the IP charge
*unless* `completed = true` was set first - which happens inside a
successful `Connections::add`, transferring ownership of the charge to
the live entry. `DisconnectFinalizer::drop` (`src/node.rs`) releases
that charge symmetrically on the teardown path.

**Failure mode.** A path that fails between `check_and_reserve` and the
guard's `completed = true` without going through the guard's `Drop`
would leak both an `ip_counts` slot and a `connecting` entry, slowly
poisoning the per-IP limit until the node refused connections from that
peer.

---

## Listener accepts are bounded by an in-flight permit semaphore

**Statement.** The number of `handle_connection_request` tasks running
concurrently never exceeds `config.max_connecting`. Inbound accepts and
outbound connects draw from a single shared setup budget
(`InnerNode::connecting_permits`); once it is exhausted, the listener
task suspends on `acquire_owned()` - holding the one connection it has
just accepted - and ceases calling `accept()` until a permit is
released, leaving surplus connections in the kernel accept queue.

**Enforcement.** `InnerNode::connecting_permits` (`src/node.rs`) is an
`Arc<Semaphore>` sized to `max_connecting`, created in `Node::new`. The
accept loop in `Node::start_listening` acquires an owned permit *after*
each successful `accept()` - never while idling in `accept`, so a quiet
listener cannot park a permit that outbound connects could use - and
moves it into the spawned `handle_connection_request` task, where it is
released on completion (success or failure). `Node::connect` takes a
permit from the same budget for the duration of its setup.

**Failure mode.** Without the semaphore, a SYN flood would let the
listener spawn unbounded handler tasks per accept, each holding a
`TcpStream` until it failed limit checks. Memory, file descriptors, and
task-table entries would grow proportional to attack volume rather than
to `max_connecting`.

---

## The node holds an internal Arc cycle; `shut_down` is required

**Statement.** A `Node` (which is `Arc<InnerNode>`) cannot drop usefully
on its own. Every spawned task - listener, protocol handlers, per-
connection reader/writer/inbound-processing tasks, detached
`on_connect` tasks under `ABORTABLE = false` - holds a clone of `Node`
through its captured `self_clone`. The inner state is reclaimed only
after `shut_down` aborts those tasks and they drop their clones.

**Enforcement.** Documented on the `Node` type in `src/node.rs`.
`shut_down` aborts the listener first, then concurrently disconnects
all `active` peers (which cascades through `Connection::Drop` to abort
all per-connection tasks), then signals the protocol handler tasks to
drain and exit, aborting them only as a timed fallback.
After `shut_down` returns, the only outstanding `Node` clones are held
by any in-flight `ABORTABLE = false` `on_connect` tasks (these complete
in bounded time provided the user's callback terminates), plus,
transiently, aborted tasks not yet reaped by the runtime.

**Failure mode.** Forgetting to call `shut_down` leaks the entire node:
the listener keeps accepting, the handler tasks keep running, sockets
stay open, and the `InnerNode` is never dropped.

---

## Protocol handler wind-down is graceful and strand-free

**Statement.** During `Node::shut_down`, no message already sent (or still
mid-send) to a protocol handler channel is lost: the setup handlers
(Handshake/Reading/Writing) fail their queued requests - each caller
observes a "shutting down" error - while the hook handlers
(OnConnect/OnDisconnect) still run their queued triggers, preserving the
OnConnect/OnDisconnect pairing. `shut_down` additionally waits for in-flight
`OnConnect` scheduling tasks before signaling: they are the only trigger
producers that are deliberately detached from their connection's setup, so
without the wait, one delayed by executor lag could fire its trigger into an
already-closed channel and silently skip the hook. `OnDisconnect` triggers
need no such gate - every disconnect owner holds the `disconnecting` claim
and only removes its connection from `active` after its trigger, so the
active-set drain already sequences them before the signal.

**Enforcement.** The phase-two handler signal in `ShutdownState`
(`src/node.rs`, a `watch` channel) plus the
drain arms in `run_setup_handler_loop` / `run_hook_handler_loop`
(`src/protocols/mod.rs`): upon the signal, each loop closes its channel and
drains it via `recv`, which - unlike dropping the receiver - waits out any
send that is still mid-write. The scheduling wait is implemented by
`SchedulingGuard` (`src/node.rs`), whose count is incremented *before* the
connection enters `active`, so `shut_down`'s active-drain cannot miss it.
`await_handler_response` (`src/protocols/mod.rs`) remains as a backstop for
the hard-abort paths (a drain timeout, or a `shut_down` future dropped
mid-execution), where a message can still be stranded in a closed channel;
it bounds the caller's wait instead of leaving it wedged.

**Failure mode.** Aborting a handler task with queued or mid-write messages
either wedges the sender forever (an in-flight `connect` that never
resolves, observed as ever-growing in-flight counts in the chaos test) or
silently skips a hook, breaking the pairing that `OnConnect::ABORTABLE =
false` promises. Both failure modes were observed under chaos-test churn
before this mechanism existed.

---

## Non-invariants

The following properties might look like they should hold but
*deliberately do not*. Code that depends on any of them is unsound.

- **`on_connect` is not guaranteed to fire in the default configuration.**
  Under high contention, the spawned task may be aborted before its
  first poll. Set `OnConnect::ABORTABLE = false` to opt into the
  guarantee; see the relevant invariant above.

- **`process_message` and `on_connect` can run concurrently on the same
  connection.** `on_connect` is spawned as its own task and runs in
  parallel with reader-driven `process_message` invocations. Any shared
  state mutated by both callbacks must be synchronized by the user.

- **Inbound message order across connections is not preserved.** Per-
  connection FIFO is preserved by the codec/framed reader, but no
  cross-connection ordering exists. Two messages from two different
  peers may be processed in any order regardless of arrival time.

- **`Node` does not implement leak-free `Drop`.** See the reference-cycle
  invariant above. `Node` is a handle to a long-lived runtime object
  whose teardown is explicit.

- **Connection identity is by `SocketAddr` only.** If peer A connects to
  peer B at the same time that peer B connects to peer A, the library
  records two independent connections (each side seeing one inbound and
  one outbound). Applications that need single-logical-connection-per-
  peer semantics must implement tie-breaking in `Handshake`.

- **The self-connect check is best-effort.** It compares against the
  node's own listening address and - for a wildcard-bound listener -
  the loopback variant of its port, but does not enumerate local
  interfaces. A wildcard-bound node connecting to one of its own
  non-loopback addresses will succeed in connecting to itself.

- **`unicast` returning `Ok` does not guarantee delivery.** The `Ok`
  return only means the message was successfully enqueued. The returned
  `oneshot::Receiver<io::Result<()>>` resolves once the message's fate
  is known: `Ok(Ok(()))` when the write batch is flushed into the OS
  socket buffer (not a peer-side acknowledgement), `Ok(Err(_))` when
  the batch fails, and `Err(RecvError)` when the connection is torn
  down before the write concludes - in which case the message may or
  may not have reached the socket.

- **`broadcast` (deprecated since 0.57.0) does not provide delivery
  confirmation.** Per-peer enqueue failures are logged and skipped, not
  returned to the caller (only the API-misuse `Unsupported` error aborts
  the call). Use `Node::connected_addrs` with `unicast` / `unicast_fast`
  - the deprecation's suggested replacement - if you need per-peer
  feedback.

---

## Verification

The chaos test (`tests/tests/chaos.rs`, run via
`cargo test -p tests --profile chaos chaos -- --ignored --nocapture`)
exercises every invariant above
under sustained random churn:

- Lock ordering is exercised by the constant interleaving of `connect`,
  `disconnect`, and `shut_down` calls across 16+ workers.
- The shutdown serialization is exercised by workers continuing to
  `connect` against a pool of nodes being concurrently shut down by
  other workers.
- Disconnect idempotency is exercised by `DisconnectOnDrop` firing
  alongside explicit user `disconnect()` calls during teardown.
- OnConnect/OnDisconnect pairing is verified numerically: with
  `ABORTABLE = false`, the test asserts exact post-cleanup equality
  across millions of paired events.
- The senders-map invariant is exercised by the rapid connect/
  disconnect cycle, which would surface as `unicast`-returns-
  `NotConnected` on otherwise-live connections if the gating broke.

A failed invariant typically surfaces as drift in the
`on_connect`/`on_disconnect` counters, a hang during cleanup, or
unbounded growth in resource usage over long runs. The chaos test's
watchdogs actively enforce the latter two - a wedged action, a
worker-wide stall, a post-`shut_down` leftover connection, or
file-descriptor/task counts above their ceilings fail the run
immediately - while the counter drift is asserted at the end of the
run. (Heap-level tracking via a counting allocator exists in the test
but is currently commented out.)
