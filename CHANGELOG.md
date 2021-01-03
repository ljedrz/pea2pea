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
