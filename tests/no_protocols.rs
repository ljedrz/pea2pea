use bytes::Bytes;

mod common;

#[tokio::test]
async fn no_protocols_usage() {
    tracing_subscriber::fmt::init();

    let nodes = common::start_inert_nodes(2, None).await;

    nodes[0]
        .initiate_connection(nodes[1].listening_addr)
        .await
        .unwrap();

    wait_until!(1, nodes[1].num_connected() == 1);

    let message = Bytes::from("I may implement nothing, but I can still send stuff out!");

    // the target node won't receive it, as it doesn't implement Messaging
    nodes[0]
        .send_direct_message(nodes[1].listening_addr, message)
        .await
        .unwrap();

    wait_until!(1, nodes[0].stats.sent().0 == 1);
}
