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
