use bytes::Bytes;
use tokio::time::sleep;

use pea2pea::Node;

use std::time::Duration;

#[tokio::test]
async fn no_protocols() {
    tracing_subscriber::fmt::init();

    let bore1 = Node::new(None).await.unwrap();
    let bore2 = Node::new(None).await.unwrap();

    bore1
        .initiate_connection(bore2.listening_addr)
        .await
        .unwrap();
    sleep(Duration::from_millis(10)).await;

    let message = Bytes::from("I may implement nothing, but I can still send stuff out!");

    // bore2 won't receive it, as it doesn't implement Messaging
    bore1
        .send_direct_message(bore2.listening_addr, message.clone())
        .await
        .unwrap();
    sleep(Duration::from_millis(10)).await;

    assert_eq!(bore1.num_messages_sent(), 1);
    assert_eq!(bore2.num_messages_received(), 0);
}
