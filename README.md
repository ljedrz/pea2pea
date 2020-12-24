# pea2pea
[![license](https://img.shields.io/badge/license-CC0-blue.svg)](https://creativecommons.org/publicdomain/zero/1.0/)
[![current version](https://img.shields.io/crates/v/pea2pea.svg)](https://crates.io/crates/pea2pea)

**pea2pea** is a P2P library designed with the following use cases in mind:
- simple and quick creation of P2P networks
- testing/verifying P2P network protocols
- benchmarking and stress-testing P2P nodes
- substituting other, "heavier" nodes in local network tests

## goals
- small, simple codebase
- ease of use
- interoperability
- good performance

## non-goals
- `no_std`
- DHT
- becoming a framework
- support for multiple `async` runtimes

## current state: _alpha_
- the basic functionalities are available and seem to be working
- the documentation is pretty much non-existent
- plenty of FIXMEs and TODOs left to iron out
- the public APIs are a bit messy and definitely subject to change
- the tests need to be extended

## how to use it
- the `tests` directory contains some examples
- `examples` coming soon
