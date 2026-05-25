mod common;

use std::{
    collections::HashSet,
    net::SocketAddr,
    sync::{Arc, LazyLock},
    time::Duration,
};

use parking_lot::Mutex;
use pea2pea::{
    Pea2Pea,
    protocols::{OnDisconnect, Reading},
};

use crate::common::wait_until;

static DISCONNECT_TRIGGERED: LazyLock<Arc<Mutex<HashSet<String>>>> =
    LazyLock::new(Default::default);

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
    connector.enable_on_disconnect().await;

    let connectee = crate::test_node!("connectee0");
    connectee.enable_reading().await;
    connectee.enable_on_disconnect().await;
    let connectee_addr = connectee.node().toggle_listener().await.unwrap().unwrap();

    connector.node().connect(connectee_addr).await.unwrap();

    wait_until(Duration::from_secs(1), || {
        connectee.node().num_connected() == 1
    })
    .await;

    let disconnects = DISCONNECT_TRIGGERED.lock().clone();
    assert!(
        !disconnects.contains(connector.node().name())
            && !disconnects.contains(connectee.node().name())
    );

    connector.node().disconnect(connectee_addr).await;

    wait_until(Duration::from_secs(1), || {
        let disconnects = DISCONNECT_TRIGGERED.lock().clone();
        disconnects.contains(connector.node().name())
            && disconnects.contains(connectee.node().name())
    })
    .await;
}

#[tokio::test]
async fn connectee_side_disconnect() {
    let connector = crate::test_node!("connector1");
    connector.enable_reading().await;
    connector.enable_on_disconnect().await;

    let connectee = crate::test_node!("connectee1");
    connectee.enable_on_disconnect().await;
    let connectee_addr = connectee.node().toggle_listener().await.unwrap().unwrap();

    connector.node().connect(connectee_addr).await.unwrap();

    wait_until(Duration::from_secs(1), || {
        connectee.node().num_connected() == 1
    })
    .await;

    let disconnects = DISCONNECT_TRIGGERED.lock().clone();
    assert!(
        !disconnects.contains(connector.node().name())
            && !disconnects.contains(connectee.node().name())
    );

    let connector_addr = connectee.node().connected_addrs()[0];
    connectee.node().disconnect(connector_addr).await;

    wait_until(Duration::from_secs(1), || {
        let disconnects = DISCONNECT_TRIGGERED.lock().clone();
        disconnects.contains(connector.node().name())
            && disconnects.contains(connectee.node().name())
    })
    .await;
}
