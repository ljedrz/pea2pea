# pea2pea
[![crates.io](https://img.shields.io/crates/v/pea2pea)](https://crates.io/crates/pea2pea)
[![docs.rs](https://docs.rs/pea2pea/badge.svg)](https://docs.rs/pea2pea)
[![LOC](https://tokei.rs/b1/github/ljedrz/pea2pea?category=code)](https://github.com/ljedrz/pea2pea/tree/master/src)
[![dependencies](https://deps.rs/repo/github/ljedrz/pea2pea/status.svg)](https://deps.rs/repo/github/ljedrz/pea2pea)
[![actively developed](https://img.shields.io/badge/maintenance-actively--developed-brightgreen.svg)](https://gist.github.com/cheerfulstoic/d107229326a01ff0f333a1d3476e068d)
[![issues](https://img.shields.io/github/issues-raw/ljedrz/pea2pea)](https://github.com/ljedrz/pea2pea/issues)

**pea2pea** is a simple, low-level, and customizable implementation of a TCP P2P node.

The core library only provides the most basic functionalities like starting, ending and maintaining connections; the rest is up to a few
low-level, opt-in [protocols](https://docs.rs/pea2pea/latest/pea2pea/protocols/index.html):
- [`Handshake`](https://docs.rs/pea2pea/latest/pea2pea/protocols/trait.Handshake.html) requires connections to adhere to a handshake protocol before finalizing connections
- [`Reading`](https://docs.rs/pea2pea/latest/pea2pea/protocols/trait.Reading.html) enables the node to receive messages based on the user-supplied [Decoder](https://docs.rs/tokio-util/latest/tokio_util/codec/trait.Decoder.html)
- [`Writing`](https://docs.rs/pea2pea/latest/pea2pea/protocols/trait.Writing.html) enables the node to send messages based on the user-supplied [Encoder](https://docs.rs/tokio-util/latest/tokio_util/codec/trait.Encoder.html)
- [`Disconnect`](https://docs.rs/pea2pea/latest/pea2pea/protocols/trait.Disconnect.html) allows the node to perform specified actions whenever a peer disconnects

## goals
- small, simple, non-framework codebase: the entire library is ~1k LOC and there are few dependencies
- ease of use: few objects and traits, no "turboeels" or generics/references that would force all parent objects to adapt
- correctness: builds with stable Rust, there is no `unsafe` code, there's more code in `tests` than in the actual library
- low-level oriented: the user has full control over all connections and every byte sent or received
- good performance: over 10GB/s in favorable scenarios, small memory footprint

## how to use it
1. define a clonable struct containing a [`Node`](https://docs.rs/pea2pea/latest/pea2pea/struct.Node.html) and any extra state you'd like to carry
2. implement the trivial [`Pea2Pea`](https://docs.rs/pea2pea/latest/pea2pea/trait.Pea2Pea.html) trait for it
3. make it implement any/all of the [protocols](https://docs.rs/pea2pea/latest/pea2pea/protocols/index.html)
4. create that struct (or as many of them as you like)
5. enable the protocols you'd like them to utilize

That's it!

## [examples](https://github.com/ljedrz/pea2pea/tree/master/examples)

- including [noise](https://noiseprotocol.org/noise.html) encryption, simple interop with [`libp2p`](https://crates.io/crates/libp2p), or [TLS](https://en.wikipedia.org/wiki/Transport_Layer_Security) connections

## status
- all the desired functionalities are complete
- the crate follows [semver](https://semver.org/), and some API breakage is still possible before `1.0`
- the project is actively developed, with all changes being recorded in the [CHANGELOG](https://github.com/ljedrz/pea2pea/blob/master/CHANGELOG.md)
- some of the desired features that are not yet available in Rust include [associated type defaults](https://github.com/rust-lang/rust/issues/29661)
- the project aims to always build with the current stable Rust compiler; legacy version support is not a goal, but they might also work
