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
