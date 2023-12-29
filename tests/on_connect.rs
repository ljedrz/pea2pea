use deadline::deadline;

mod common;
use std::{net::SocketAddr, time::Duration};

use pea2pea::{
    protocols::{OnConnect, Reading, Writing},
    Pea2Pea,
};

use crate::common::WritingExt;

impl OnConnect for common::TestNode {
    async fn on_connect(&self, peer_addr: SocketAddr) {
        self.send_dm(peer_addr, "connected".into()).await.unwrap();
    }
}

#[tokio::test]
async fn on_connect_message() {
    let connector = crate::test_node!("connector");
    connector.enable_writing().await;
    connector.enable_on_connect().await;

    let connectee = crate::test_node!("connectee");
    connectee.enable_reading().await;
    let connectee_addr = connectee.node().start_listening().await.unwrap();

    connector.node().connect(connectee_addr).await.unwrap();

    let connectee_clone = connectee.clone();
    deadline!(Duration::from_secs(1), move || connectee_clone
        .node()
        .num_connected()
        == 1);

    deadline!(Duration::from_secs(1), move || {
        connectee.node().stats().received().0 == 1
    });
}
