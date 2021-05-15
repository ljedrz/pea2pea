# 0.20.0

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
