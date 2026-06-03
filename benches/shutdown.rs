use std::{io, net::SocketAddr};

use divan::Bencher;
use pea2pea::{
    Node, Pea2Pea,
    protocols::{Handshake, OnDisconnect, Reading, Writing},
};
use tokio::runtime::Runtime;

#[path = "../tests/common/mod.rs"]
mod common;
use common::TestNode;

fn main() {
    divan::main();
}

// `common` already gives `TestNode` its `Reading`/`Writing` (and `OnConnect`)
// impls; add the no-op `Handshake`/`OnDisconnect` ones so every protocol can be
// enabled below.
impl_noop_disconnect_and_handshake!(common::TestNode);

/// Time to tear a fully-featured, listening node down.
///
/// The node is built and brought up in `with_inputs` (untimed); only the
/// `shut_down` call - aborting the listener and protocol tasks and releasing
/// the socket - is measured. `sample_size` is 1 so divan only ever has a single
/// started node (and thus a single listener fd) live at a time.
#[divan::bench(sample_count = 200, sample_size = 1)]
fn node_shutdown(bencher: Bencher) {
    let rt = runtime();

    bencher
        .with_inputs(|| {
            // untimed: stand up a fully-featured, listening node
            rt.block_on(async {
                let node = TestNode {
                    node: Node::new(Default::default()),
                    barrier: Default::default(),
                };

                node.enable_handshake().await;
                node.enable_reading().await;
                node.enable_writing().await;
                node.enable_on_disconnect().await;
                node.node().toggle_listener().await.unwrap().unwrap();

                node
            })
        })
        .bench_local_values(|node| {
            // timed: only the shut-down sequence
            rt.block_on(async move {
                node.node().shut_down().await;
            });
        });
}

fn runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}
