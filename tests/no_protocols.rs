use bytes::Bytes;

mod common;
use pea2pea::Node;

#[tokio::test]
async fn no_protocols_usage() {
    tracing_subscriber::fmt::init();

    let bore1 = Node::new(None).await.unwrap();
    let bore2 = Node::new(None).await.unwrap();

    bore1
        .initiate_connection(bore2.listening_addr)
        .await
        .unwrap();

    wait_until!(1, bore2.num_handshaken() == 1);

    let message = Bytes::from("I may implement nothing, but I can still send stuff out!");

    // bore2 won't receive it, as it doesn't implement Messaging
    bore1
        .send_direct_message(bore2.listening_addr, message)
        .await
        .unwrap();

    wait_until!(
        1,
        bore1.num_messages_sent() == 1 && bore2.num_messages_received() == 0
    );
}
