# pea2pea
[![license](https://img.shields.io/badge/license-CC0-blue)](https://creativecommons.org/publicdomain/zero/1.0/)
[![current version](https://img.shields.io/crates/v/pea2pea)](https://crates.io/crates/pea2pea)
[![files](https://tokei.rs/b1/github/ljedrz/pea2pea?category=files)](https://github.com/ljedrz/pea2pea/tree/master/src)
[![LOC](https://tokei.rs/b1/github/ljedrz/pea2pea?category=code)](https://github.com/ljedrz/pea2pea/tree/master/src)
[![build status](https://img.shields.io/travis/com/ljedrz/pea2pea)](https://travis-ci.com/ljedrz/pea2pea)

**pea2pea** is a P2P library designed with the following use cases in mind:
- simple and quick creation of custom P2P networks
- testing/verifying network protocols
- benchmarking and stress-testing P2P nodes (or other network entities)
- substituting other, "heavier" nodes in local network tests

## goals
- small, simple codebase: the core is under 1k LOC; most of the library are tests and examples
- ease of use: few objects and traits, no "turboeels" or generics/references forcing all parent objects to adapt
- interoperability: strives to be as versatile as possible without sacrificing simplicity and ease of use
- good performance: over 1GB/s in favorable scenarios, small memory footprint

## non-goals
- `no_std`
- becoming a framework
- support for multiple `async` runtimes (it should be simple enough to change it, though)
- any functionality that can be introduced "on top" (e.g. DHT, advanced topology formation algorithms etc.)

## how to use it
1. define a clonable struct containing a `Node` and any extra state you'd like to carry
2. `impl Pea2Pea` for it
3. make it implement any/all of the protocols
4. create that struct (or as many of them as you like)
5. enable protocols you'd like the node(s) to utilize

That's it!

## examples

- the [tests](https://github.com/ljedrz/pea2pea/tree/master/tests) directory contains some examples of simple use
- [examples](https://github.com/ljedrz/pea2pea/tree/master/examples) contain more advanced setups, e.g. using [noise](https://noiseprotocol.org/noise.html) encryption
- try running `cargo run --example <example_name>` with different `RUST_LOG` verbosity levels to check out what's going on under the hood
