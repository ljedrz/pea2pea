# pea2pea
[![license](https://img.shields.io/badge/license-CC0-blue.svg)](https://creativecommons.org/publicdomain/zero/1.0/)
[![current version](https://img.shields.io/crates/v/pea2pea.svg)](https://crates.io/crates/pea2pea)

**pea2pea** is a P2P library designed with the following use cases in mind:
- simple and quick creation of P2P networks
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
- DHT (it _can_ be applied on top, however)
- becoming a framework
- support for multiple `async` runtimes

## current state: _alpha_
- the basic functionalities are available and seem to be working
- the documentation is pretty much non-existent
- plenty of FIXMEs and TODOs left to iron out
- the public APIs are a bit messy and definitely subject to change
- the tests need to be extended

## how to use it
1. define a custom struct containing an `Arc<Node>` and any extra state you'd like to carry
2. `impl ContainsNode` for it
3. make it implement any/all of the protocols
4. create that struct (or as many of them as you like)
5. `enable_X_protocol` for all the protocols you want it to activate

that's it!

- the `tests` directory contains some examples of simple use
- `examples` contain more advanced setups, e.g. using [noise](https://noiseprotocol.org/noise.html) encryption
