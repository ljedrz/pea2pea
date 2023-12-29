use deadline::deadline;
use once_cell::sync::Lazy;
use parking_lot::Mutex;

mod common;
use std::{collections::HashSet, net::SocketAddr, sync::Arc, time::Duration};

use pea2pea::{
    protocols::{OnDisconnect, Reading},
    Pea2Pea,
};

static DISCONNECT_TRIGGERED: Lazy<Arc<Mutex<HashSet<String>>>> = Lazy::new(Default::default);

impl OnDisconnect for common::TestNode {
    async fn on_disconnect(&self, _addr: SocketAddr) {
        DISCONNECT_TRIGGERED
            .lock()
            .insert(self.node().name().to_owned());
    }
}

#[tokio::test]
async fn connector_side_disconnect() {
    let connector = crate::test_node!("connector0");
    connector.enable_disconnect().await;

    let connectee = crate::test_node!("connectee0");
    connectee.enable_reading().await;
    connectee.enable_disconnect().await;
    let connectee_addr = connectee.node().start_listening().await.unwrap();

    connector.node().connect(connectee_addr).await.unwrap();

    let connectee_clone = connectee.clone();
    deadline!(Duration::from_secs(1), move || connectee_clone
        .node()
        .num_connected()
        == 1);

    let disconnects = DISCONNECT_TRIGGERED.lock().clone();
    assert!(
        !disconnects.contains(connector.node().name())
            && !disconnects.contains(connectee.node().name())
    );

    connector.node().disconnect(connectee_addr).await;

    deadline!(Duration::from_secs(1), move || {
        let disconnects = DISCONNECT_TRIGGERED.lock().clone();
        disconnects.contains(connector.node().name())
            && disconnects.contains(connectee.node().name())
    });
}

#[tokio::test]
async fn connectee_side_disconnect() {
    let connector = crate::test_node!("connector1");
    connector.enable_reading().await;
    connector.enable_disconnect().await;

    let connectee = crate::test_node!("connectee1");
    connectee.enable_disconnect().await;
    let connectee_addr = connectee.node().start_listening().await.unwrap();

    connector.node().connect(connectee_addr).await.unwrap();

    let connectee_clone = connectee.clone();
    deadline!(Duration::from_secs(1), move || connectee_clone
        .node()
        .num_connected()
        == 1);

    let disconnects = DISCONNECT_TRIGGERED.lock().clone();
    assert!(
        !disconnects.contains(connector.node().name())
            && !disconnects.contains(connectee.node().name())
    );

    let connector_addr = connectee.node().connected_addrs()[0];
    connectee.node().disconnect(connector_addr).await;

    deadline!(Duration::from_secs(1), move || {
        let disconnects = DISCONNECT_TRIGGERED.lock().clone();
        disconnects.contains(connector.node().name())
            && disconnects.contains(connectee.node().name())
    });
}
