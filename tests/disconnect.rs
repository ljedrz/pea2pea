use bytes::Bytes;

mod common;
use crate::common::WritingExt;
use pea2pea::{
    protocols::{Disconnect, Reading, Writing},
    Pea2Pea,
};

use std::net::SocketAddr;

#[async_trait::async_trait]
impl Disconnect for common::TestNode {
    async fn handle_disconnect(&self, addr: SocketAddr) {
        let disconnect_message = Bytes::from("bye-bye!".as_bytes());

        self.send_dm(addr, disconnect_message).await.unwrap();
    }
}

#[tokio::test]
async fn send_message_before_disconnect() {
    let connector = crate::test_node!("connector");
    connector.enable_writing().await;
    connector.enable_disconnect().await;

    let connectee = crate::test_node!("connectee");
    connectee.enable_reading().await;

    let connectee_addr = connectee.node().listening_addr().unwrap();

    connector.node().connect(connectee_addr).await.unwrap();

    wait_until!(1, connectee.node().num_connected() == 1);

    assert_eq!(connectee.node().stats().received().0, 0);

    connector.node().disconnect(connectee_addr).await;

    wait_until!(1, connectee.node().stats().received().0 == 1);
}
