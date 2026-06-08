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

/// Time to bring a fully-featured node up: construct it, enable every protocol,
/// and start its listener.
///
/// Each measured node is handed off to a background task for shutdown, so the
/// timed region covers only start-up while the listener socket is still
/// reclaimed promptly off the hot path. `sample_size` is 1 so at most a node or
/// two is ever live at once, keeping clear of any open-file limit.
#[divan::bench(sample_count = 200, sample_size = 1)]
fn node_startup(bencher: Bencher) {
    let rt = runtime();

    bencher.bench_local(|| {
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

            // tear the node down off the timed path so its listener fd doesn't
            // linger, without charging shutdown to the measurement
            tokio::spawn(async move { node.node().shut_down().await });
        });
    });
}

fn runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}
