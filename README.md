# pea2pea
[![license](https://img.shields.io/badge/license-CC0-blue.svg)](https://creativecommons.org/publicdomain/zero/1.0/)
[![current version](https://img.shields.io/crates/v/pea2pea.svg)](https://crates.io/crates/pea2pea)

**pea2pea** is a P2P library designed with the following use cases in mind:
- simple and quick creation of custom P2P networks
- testing/verifying network protocols
- benchmarking and stress-testing P2P nodes (or other network entities)
- substituting other, "heavier" nodes in local network tests

## goals
- small, simple codebase
- ease of use
- interoperability
- good performance

## non-goals
- `no_std`
- becoming a framework
- support for multiple `async` runtimes (it should be simple enough to change it, though)
- any functionality that can be introduced "on top" (e.g. DHT, advanced topology formation algorithms etc.)

## current state: _beta_
- all the planned functionalities are available and operational
- benchmarks demonstrate _very_ good performance (over 1GB/s in favorable scenarios)
- the public APIs are unstable
- more tests, benches and examples wouldn't hurt

## how to use it
1. define a clonable struct containing an `Arc<Node>` and any extra state you'd like to carry
2. `impl ContainsNode` for it
3. make it implement any/all of the protocols
4. create that struct (or as many of them as you like)
5. enable protocols you'd like the node(s) to utilize

that's it!

- the `tests` directory contains some examples of simple use
- `examples` contain more advanced setups, e.g. using [noise](https://noiseprotocol.org/noise.html) encryption
