use divan::Bencher;
use pea2pea::{Pea2Pea, protocols::Reading};
use tokio::runtime::Runtime;

#[path = "../tests/common/mod.rs"]
mod common;

fn main() {
    divan::main();
}

fn runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// A bare connect + disconnect cycle, measured from the initiator's side.
///
/// The responder only enables `Reading` so that it tears down its half of the
/// connection once the initiator drops it; that keeps its open file descriptors
/// bounded for the duration of the run. No handshake or other protocol is
/// involved, so this is the cost of pea2pea's own connection setup/teardown on
/// top of a loopback TCP connect, nothing more.
///
/// The sample size is capped deliberately: a tight connect/disconnect loop
/// against a *fixed* responder address piles up `TIME_WAIT` entries on the
/// initiator's ephemeral ports, and once those are exhausted `connect` starts
/// failing with `EADDRNOTAVAIL`. `sample_count * sample_size` outbound
/// connections stays comfortably within the ephemeral range.
#[divan::bench(sample_count = 100, sample_size = 30)]
fn connect_disconnect(bencher: Bencher) {
    let rt = runtime();

    // Untimed setup: one long-lived responder + initiator pair, reused across
    // every sample.
    let (initiator, responder, responder_addr) = rt.block_on(async {
        let responder = test_node!("responder");
        responder.enable_reading().await;
        let responder_addr = responder.node().toggle_listener().await.unwrap().unwrap();

        let initiator = test_node!("initiator");

        (initiator, responder, responder_addr)
    });

    bencher.bench_local(|| {
        rt.block_on(async {
            initiator.node().connect(responder_addr).await.unwrap();
            initiator.node().disconnect(responder_addr).await;
        });
    });

    rt.block_on(async {
        initiator.node().shut_down().await;
        responder.node().shut_down().await;
    });
}
