# pea2pea
[![license](https://img.shields.io/badge/license-CC0-blue.svg)](https://creativecommons.org/publicdomain/zero/1.0/)
[![current version](https://img.shields.io/crates/v/pea2pea.svg)](https://crates.io/crates/pea2pea)

**pea2pea** is a P2P library designed with the following use cases in mind:
- simple and quick creation of custom P2P networks
- testing/verifying network protocols
- benchmarking and stress-testing P2P nodes (or other network entities)
- substituting other, "heavier" nodes in local network tests

The benchmarks demonstrate pretty good performance (over 1GB/s in favorable scenarios), and the resource use is quite
low; if you start a LOT of nodes, the only limit you'll encounter is most likely going to be the file descriptor one
(due to every node opening a TCP socket to listen for connection requests).

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

## how to use it
1. define a clonable struct containing an `Arc<Node>` and any extra state you'd like to carry
2. `impl Pea2Pea` for it
3. make it implement any/all of the protocols
4. create that struct (or as many of them as you like)
5. enable protocols you'd like the node(s) to utilize

That's it!

- the `tests` directory contains some examples of simple use
- `examples` contain more advanced setups, e.g. using [noise](https://noiseprotocol.org/noise.html) encryption
- try different `RUST_LOG` verbosity levels to check out what's going on under the hood

## example

Creating and starting 2 nodes, one of which writes and the other reads textual messages prefixed with their length:

```rust
use async_trait::async_trait;
use pea2pea::{protocols::{Reading, Writing}, Node, NodeConfig, Pea2Pea};
use tokio::time::sleep;
use tracing::*;

use std::{convert::TryInto, io::{ErrorKind, Result}, net::SocketAddr, sync::Arc, time::Duration};

#[derive(Clone)]
struct ExampleNode(Arc<Node>);

impl Pea2Pea for ExampleNode {
    fn node(&self) -> &Arc<Node> { &self.0 }
}

impl ExampleNode {
    async fn new(name: &str) -> Self {
        let config = NodeConfig { name: Some(name.into()), ..Default::default() };
        ExampleNode(Node::new(Some(config)).await.unwrap())
    }
}

#[async_trait]
impl Reading for ExampleNode {
    type Message = String;

    fn read_message(&self, source: SocketAddr, buffer: &[u8]) -> Result<Option<(String, usize)>> {
        debug!(parent: self.node().span(), "attempting to read a message from {}", source);

        // expecting incoming messages to be prefixed with their length encoded as a LE u16
        if buffer.len() >= 2 {
            let payload_len = u16::from_le_bytes(buffer[..2].try_into().unwrap()) as usize;

            if payload_len == 0 { return Err(ErrorKind::InvalidData.into()); }

            if buffer[2..].len() >= payload_len {
                let message = String::from_utf8(buffer[2..][..payload_len].to_vec())
                    .map_err(|_| ErrorKind::InvalidData)?;

                Ok(Some((message, 2 + payload_len)))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    async fn process_message(&self, source: SocketAddr, message: String) -> Result<()> {
        info!(parent: self.node().span(), "received a message from {}: {}", source, message);
        Ok(())
    }
}

impl Writing for ExampleNode {
    fn write_message(&self, _target: SocketAddr, payload: &[u8], buffer: &mut [u8]) -> Result<usize> {
        assert!(buffer.len() >= payload.len() + 2);
        buffer[..2].copy_from_slice(&(payload.len() as u16).to_le_bytes());
        buffer[2..][..payload.len()].copy_from_slice(&payload);
        Ok(2 + payload.len())
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let alice = ExampleNode::new("Alice").await;
    let bob = ExampleNode::new("Bob").await;
    let bobs_addr = bob.node().listening_addr;

    alice.enable_writing();
    bob.enable_reading();

    alice.node().connect(bobs_addr).await.unwrap();
    alice.node().send_direct_message(bobs_addr, b"Hello there!"[..].into()).await.unwrap();

    sleep(Duration::from_millis(10)).await; // a small delay to allow all the logs to be displayed
}
```
