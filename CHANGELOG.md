# 0.49.0

### Added

- `Writing::INITIAL_BUFFER_SIZE`

### Changed

- renamed `Config::listener_ip` to `::listener_addr`
- dropped the dependency on `async_trait` in favor of Rust 1.75 features
- bumped the MSRV to 1.75

### Removed

- `Config::{allow_random_port, desired_listening_port}`

# 0.48.0

### Changed

- `once_cell` is now only a dev dependency
- due to the use of `std::sync::OnceLock`, the MSRV is increased to 1.70
- `protocols::Disconnect` was renamed to `::OnDisconnect`
- `Disconnect::handle_disconnect` is now `OnDisconnect::on_disconnect`

# 0.47.0

### Added

- `Config::connection_timeout_ms`

# 0.46.0

### Added

- `protocols::OnConnect`

# 0.45.0

### Changed

- increased the minimum required version of `tokio` to `1.24` due to RUSTSEC-2023-0001

# 0.44.0

### Added

- `ConnectionInfo`, which contains basic information related to an active connection
- `Node::{connection_info, connection_infos}`, which return `ConnectionInfo` for the given address or for all of the active connections
- `Stats::created`, which registers the creation time of the node and the connections

### Changes

- the `Stats` no longer track generic failures

### Removed

- `KnownPeers` (in favor of `ConnectionInfo`, which contains `Stats`)
- `Node::known_peers`
- `Stats::failures`

# 0.43.0

### Added

- `Node::connect_using_socket`, which can be used to create outbound connections using pre-configured, user-supplied sockets
- `Node::start_listening` which makes the node listen for inbound connections

### Changed

- the nodes no longer start listening for inbound connections automatically
- `Node::new` is no longer `async` and doesn't return an `io::Result` anymore

### Removed

- `Config::bound_addr` (`Node::connect_using_socket` is much more versatile)

# 0.42.0

### Added

- `Config::bound_addr`

# 0.41.0

### Changed

- downgraded the `WARN` log in case `Node::disconnect` is called on a nonexistent connection to `DEBUG` and made it more clear

### Fixed

- `Reading` no longer triggers an additional noop call to `Node::disconnect` in case of read errors
- in some rare cases, it was possible for protocol handler tasks to not be able to notify their owning tasks that they were complete; this is now handled gracefully
- the same was done with connection handlers

# 0.40.0

### Changed

- `Node::new` now takes `Config` instead of `Option<Config>`
- renamed `Writing::{send_direct_message, send_broadcast}` to `::{unicast, broadcast}`

# 0.39.0

### Changed

- `Reading::codec` and `Writing::codec` now also have a `side` parameter

# 0.38.0

### Added

- `Handshake::{take_stream, return_stream}`

# 0.37.0

### Changed

- `Config::{inbound_queue_depth, initial_read_buffer_size}` were moved to `Reading::{MESSAGE_QUEUE_DEPTH, INITIAL_BUFFER_SIZE}`
- `Config::max_handshake_time_ms` was moved to `Handshake::TIMEOUT_MS`
- `Config::outbound_queue_depth` was moved to `Writing::MESSAGE_QUEUE_DEPTH`
- `Writing::send_direct_message` now returns an `io::Result<oneshot::Receiver<io::Result<()>>>` instead of `io::Result<oneshot::Receiver<bool>>`
- several objects and trait methods indended to not be used outside of the crate were made private

# 0.36.0

### Changed

- `Reading::map_codec` is now generic over `AsyncRead`
- use `OnceBox` instead of `OnceCell` in the protocols
- the `Reading` protocol should now be a bit faster to start reading

### Fixed

- nodes that implement neither `Reading` nor `Writing` no longer instantly disconnect from peers

### Removed

- `Config::invalid_read_delay_secs` (not that practical)
- `Reading::take_reader`
- `Writing::take_writer`
- the fuzz tests (as the buffering is outsourced now)

# 0.35.1

### Fixed

- fixed a panic when a protocol handler can't be triggered

### Changed

- some protocol plumbing (shouldn't affect public APIs)

# 0.35.0

### Added

- `Connection::{addr, side}`
- `Handshake::{borrow_stream}`
- `Reading::{codec, take_reader, Codec}`
- `Writing::{codec, take_writer, Codec}`

### Changed

- added `PartialEq` and `Eq` to `ConnectionSide`
- the `Reading` and `Writing` protocols now take advantage of `tokio-util`'s `Decoder` and `Encoder` traits
- the signature od `Writing::write_to_stream` has changed
- the protocol handler objects and `ReturnableItem` are no longer `pub` (only `pub(crate)`)
- `Connection`'s fields are no longer `pub`

### Removed

- `Connection::{reader, writer}`
- `Reading::{process_buffer, read_from_stream, read_message}`
- `Writing::write_message`
- the `handshaking` test (the `noise_handshake` example is much more comprehensive)

# 0.34.0

### Added

- a new method, `Node::is_connecting`

### Changed

- removed `Config::protocol_handler_queue_depth`

# 0.33.0

### Added

- a `cargo-fuzz` powered fuzz test
- extended the docs and added links
- MSRV in the TOML

### Changed

- bumped the `snow` dev-dependency to `0.9`
- marked some obvious inlining targets as `#[inline]`
- made `Writing::send_broadcast` return an `io::Result<()>` instead of nothing
- made `Writing::send_direct_message` return an `io::Result<oneshot::Receiver<bool>>` instead of `io::Result<()>`

### Fixed

- made `Writing::send_direct_message` return an `io::ErrorKind::NotConnected` if the given address is not connected

# 0.32.0

### Changed

- bumped the `parking_lot` dependency to `0.12`

# 0.31.0

### Changed

- improved the clarity of a few logs
- `Node::new` will now conclude only once the listening task has started (if listening is enabled)
- made protocol-enabling methods async to have better control over their progress

### Fixed

- made `Node::listening_addr` return the configured IP (instead of always the local one)

# 0.30.0

### Changed

- updated Rust edition to 2021

# 0.29.0

### Changed

- increased the minimum required version of `tokio` to `1.14`
- increased the minimum required version of `tracing-subscriber` (dev-dependency) to `0.3`

### Fixed

- some new `clippy` lints

# 0.28.0

### Changed

- `Reading::process_message` no longer has a default dummy impl

# 0.27.1

### Changed

- set the `tokio` version to `1.10` due to the `RUSTSEC-2021-0072` security advisory

# 0.27.0

### Changed

- moved `Node::{send_broadcast, send_direct_message}` to `Writing`
- the way the protocols are structured
- renamed `Handshaking` to `Handshake`

### Removed

- the `bytes` dependency (no longer mandatory)

# 0.26.1

### Fixed

- an off-by-one in `Reading::read_from_stream` found by fuzzing

# 0.26.0

### Changed

- the signature of `Reading::read_message` was simplified
- `Reading::read_message` errors can now be non-fatal too

# 0.25.0

### Changed

- the connection buffers are now elastic/growable, i.e. they don't immediately allocate their configured size
- `Writing::write_message` now expects an `io::Write` instead of a `&mut [u8]`, and it returns `io::Result<()>`
- the `NodeConfig` members with names starting with `conn_` lose this prefix
- `NodeConfig` was renamed to `Config`

### Removed

- the `fxhash` dependency (no difference in performance)
- `NodeConfig::conn_write_buffer_size` (no longer needed or useful)

# 0.24.0

### Changed

- the `Connection` no longer carries a reference to the `Node`
- the `ReturnableConnection` type alias was replaced with a more generic `ReturnableItem<T, U>`

### Removed

- an `ERROR` log when a write is attempted with `Writing` protocol disabled

# 0.23.0

### Added

- `KnownPeers::{get, snapshot}`
- the `Node` now tracks its internal failures (currently full channel errors)

### Changed

- the node no longer has to listen for connections
- some `ErrorKind::Other` errors were made more specific
- merged `NodeStats` with `PeerStats`

### Removed

- the peer stats no longer follow the number of connections
- `PeerStats` and `NodeStats` (merged into `Stats`)
- `KnownPeers::{read, register_connection, write}`

# 0.22.0

### Added

- a new protocol, `Disconnect`

### Changed

- `Node::send_broadcast` doesn't return a value anymore
- the node no longer waits when a per-connection inbound queue is full; the message gets dropped
- downgraded the disconnect log from `INFO` to `DEBUG`

# 0.21.1

### Fixed

- 2 `ERROR` logs now display the missing node id

# 0.21.0

### Changed

- `Node::{send_direct_message, send_broadcast}` are now non-async
- `NodeConfig::conn_outbound_queue_depth` was increased from `16` to `64`

# 0.20.3

### Fixed

- the node will now recognize a zero-read as EOF and break the related connection

# 0.20.2

### Fixed

- stream read errors are now properly checked against `NodeConfig::fatal_io_errors`

# 0.20.1

### Fixed

- removed one redundant `connecting` registration when initiating connections
- the node is now guarded against connection spam by making connection responses concurrent

# 0.20.0

### Added

- `Node::num_connecting`

### Changed

- use atomics in `PeerStats`

### Fixed

- improved the accounting of connecting (but not yet connected) peers

### Removed

- `PeerStats.{added, last_connected}` - not super-interesting

# 0.19.1

### Fixed

- `KnownPeers::register_failure` won't overflow anymore, as it is saturating at its maximum

# 0.19.0

### Changed

- the default `tokio` runtime feature is now just `rt` (which is compatible with `rt-multi-thread`)
- the `ProtocolHandler` is now created from just a `mpsc::Sender<ReturnableConnection>`

# 0.18.1

### Fixed

- the `tokio` dependency has the `rt-multi-thread` feature enabled by default in order to not fail to build in crates that don't use it on their own

# 0.18.0

### Added

- `NodeConfig.fatal_io_errors` that specifies which IO errors should break connections

### Changed

- the list of read errors considered fatal is longer
- connections are now broken on write errors considered fatal too
- the `io::ErrorKind`s returned by `Node::connect` are more specific

# 0.17.2

### Changed

- the default value for `NodeConfig.conn_inbound_queue_depth` is now 64 instead of 256
- adjusted dependency features further

# 0.17.1

### Added

- (internal) automatically test for potential memleaks

### Changed

- adjusted dependency features

# 0.17.0

### Added

- `Handshaking::perform_handshake` that has no default implementation
- `connections::Connection` is now re-exported

### Changed
- `Handshaking` now requires `Self: Clone + Send + Sync + 'static`
- `Handshaking::enable_handshaking` now has a default implementation
- `NodeConfig.max_protocol_setup_time_ms` is now `NodeConfig.max_handshake_time_ms` and only applies to `Handshaking`

### Removed
- `NodeStats.connections` and the related `NodeStats::register_connection` (unused)

# 0.16.4

### Fixed
- the `fixed_length_crusaders` example needed a bump to the default `max_protocol_setup_time_ms` introduced in `0.16.2`

# 0.16.3

### Added
- `connections::ConnectionSide` is now re-exported, i.e. it can be imported with `use pea2pea::ConnectionSide`

# 0.16.2

### Added
- `NodeConfig.max_protocol_setup_time_ms`, which restricts the amount of time that a connection can take to enact all its protocols; set to 3s by default

### Fixed
- the reading loop in the default `Reading::enable_reading` impl is now delayed until the connection is fully established

# 0.16.1

### Changed
- in the default `Reading::enable_reading` impl, the message processing task is now spawned before the message reading task

### Fixed
- there is now an additional anti-self-connect safeguard in `Node::connect`

# 0.16.0

### Changed
- `Reading::read_from_stream` and `Writing::write_to_stream` are now generic over `AsyncRead` and `AsyncWrite` respectively and their arguments were extended
- `Reading::read_from_stream` now returns `io::Result<usize>` instead of `io::Result<()>`
- `Connection.reader` now contains `Option<OwnedReadHalf>`
- `Connection.writer` now contains `Option<OwnedWriteHalf>`

### Removed
- `connections::{ConnectionReader, ConnectionWriter}`

# 0.15.0

### Added
- `NodeConfig.listener_ip`

### Changed
- the default IP the node listens at is now `Ipv4Addr::UNSPECIFIED` instead of `Ipv4Addr::LOCALHOST`

# 0.14.0

### Changed
- `ReadingHandler` and `WritingHandler` are now a common `ProtocolHandler` applicable also to `Handshaking`
- `NodeConfig.{reading_handler_queue_depth, writing_handler_queue_depth}` were merged into `.protocol_handler_queue_depth`
- instead of `Node`, keep a copy of its `Span` in `ConnectionReader` & `ConnectionWriter`
- renamed `NodeConfig.invalid_message_penalty_secs` to `.invalid_read_delay_secs`
- `Node::{reading_handler, writing_handler}` methods are now private

### Fixed
- `Node::shutdown` now also shuts down the handshaking task if `Handshaking` is enabled
- the `Node` no longer panics on attempts to send to dying connections
- corrected the doc on `NodeConfig.invalid_message_penalty_secs` (now `.invalid_read_delay_secs`)

# 0.13.1

### Changed
- the `Node` can no longer connect to its own listening address

# 0.13.0

### Changed
- exposed previously public `Node` members as public methods
- `Connection` now drops its tasks in reverse order

# 0.12.0

### Added
- `Node::shut_down` that performs a full shutdown of the node

### Changed
- `Pea2Pea::node` now returns a `&Node`
- tweaked a few logs

### Fixed
- `Connection`s now shut their associated tasks down when dropped
- critical protocol failures no longer cause panics in their main tasks
- some edge cases in protocol handling

# 0.11.0

### Added
- `NodeConfig.max_connections` with a default value of 100

### Changed
- updading `PeerStats` no longer implicitly adds an entry

### Fixed
- `PeerStats.last_connected` no longer shows a timestamp if there were no connections
- `PeerStats.times_connected` no longer shows one extra connection
- `Node::connect` is now guarded against overlapping connection attempts

### Removed
- `NodeConfig.max_allowed_failures` - not used internally, not necessarily needed, easy to handle on user side
- `PeerStats.last_seen` - ditto

# 0.10.0

### Added
- `Writing::write_to_stream` with a default implementation
- a new example with fixed-length messages
- average throughput is now displayed at the end of the benchmark in `benches`

### Changed
- renamed `Node::initiate_connection` to `::connect`
- `Writing::write_message` now requires a `SocketAddr` and `&mut [u8]` instead of `&mut ConnectionWriter`

# 0.9.0

### Added
- a test with a duplicate connection
- an example in the README

### Changed
- refactored code around establishing connections
- `Connections::disconnect` is now `::remove`
- `NodeStats::register_connection` now happens as soon as the connection is barely established
- `KnownPeers::add` doesn't occur before the connection is fully established
- `Connection` now carries its task handles in a single `Vec`
- `Connection` no longer uses `OnceCell` internally (`Option` is used instead)
- `Connection` now carries `ConnectionReader` and `ConnectionWriter` while the procols are being enabled
- `HandshakingObjects`, `ReadingObjects`, and `WritingObjects` are now merged into `ReturnableConnection`

### Removed
- `Connection::send_message` (unused)

# 0.8.1

### Added
- improved and extended some logs
- added more comments

### Changed
- a small refactoring in `Node::new`

### Fixed
- the listening task handle is now kept within the `Node`
- a new `clippy` lint introduced in Rust 1.49

# 0.8.0

### Added
- `ConnectionWriter` object was isolated from `Connection`
- `Writing` protocol was introduced
- `NodeConfig` has gained some new fields

### Changed
- the crate's structure and exports were overhauled
- `connection` module was merged into `connections`
- `Messaging` protocol became `Reading`
- `InboundHandler` became `ReadingHandler`
- `MessagingObjects` became `ReadingObjects`

### Fixed
- protocol handlers now carry handles to the tasks they have spawned
