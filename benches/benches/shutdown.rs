use divan::Bencher;
use pea2pea::{
    Node, Pea2Pea,
    protocols::{Handshake, OnConnect, OnDisconnect, Reading, Writing},
};
use tokio::runtime::Runtime;

mod common;
use common::FullNoopNode;

fn main() {
    divan::main();
}

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
                let node = FullNoopNode(Node::new(Default::default()));

                let (.., listener) = tokio::join!(
                    node.enable_handshake(),
                    node.enable_reading(),
                    node.enable_writing(),
                    node.enable_on_connect(),
                    node.enable_on_disconnect(),
                    node.node().toggle_listener(),
                );
                let _ = listener.unwrap().unwrap();

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
