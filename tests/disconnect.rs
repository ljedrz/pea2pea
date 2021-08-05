mod common;
use pea2pea::{
    protocols::{Disconnect, Reading, Writing},
    Pea2Pea,
};
use tokio::time::sleep;

use std::{net::SocketAddr, time::Duration};

#[async_trait::async_trait]
impl Disconnect for common::MessagingNode {
    async fn handle_disconnect(&self, addr: SocketAddr) {
        let disconnect_message = "bye-bye!";
        let bytes = common::prefix_with_len(4, disconnect_message.as_bytes());

        self.node().send_direct_message(addr, bytes).unwrap();

        // a small delay so that the connection isn't severed too quickly
        // and the disconnect message can get processed by the recipient
        sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn send_message_before_disconnect() {
    tracing_subscriber::fmt::init();

    let connector = common::MessagingNode::new("connector").await;
    connector.enable_writing();
    connector.enable_disconnect();

    let connectee = common::MessagingNode::new("connectee").await;
    connectee.enable_reading();

    let connectee_addr = connectee.node().listening_addr();

    connector.node().connect(connectee_addr).await.unwrap();

    wait_until!(1, connectee.node().num_connected() == 1);

    assert_eq!(connectee.node().stats().received().0, 0);

    connector.node().disconnect(connectee_addr).await;

    wait_until!(1, connectee.node().stats().received().0 == 1);
}
