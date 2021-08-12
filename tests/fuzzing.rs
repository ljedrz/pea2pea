use rand::{distributions::Standard, rngs::SmallRng, Rng, SeedableRng};

mod common;
use pea2pea::{
    protocols::{Reading, Writing},
    Config, Node, Pea2Pea,
};

#[tokio::test(flavor = "multi_thread")]
async fn fuzzing() {
    const MAX_MSG_SIZE: usize = 1024 * 1024;

    let config = Config {
        read_buffer_size: MAX_MSG_SIZE,
        ..Default::default()
    };
    let tester = common::MessagingNode(Node::new(Some(config)).await.unwrap());
    tester.enable_reading();

    let sender = common::MessagingNode(Node::new(None).await.unwrap());
    sender.enable_writing();

    sender
        .node()
        .connect(tester.node().listening_addr().unwrap())
        .await
        .unwrap();

    wait_until!(1, tester.node().num_connected() == 1);

    let mut rng = SmallRng::from_entropy();

    for _ in 0..1_000 {
        let random_len: usize = rng.gen_range(1..=MAX_MSG_SIZE - 4); // account for the length prefix
        let random_payload: Vec<u8> = (&mut rng).sample_iter(Standard).take(random_len).collect();
        // ignore full outbound queue channel errors
        let _ = sender.node().send_direct_message(
            tester.node().listening_addr().unwrap(),
            random_payload.into(),
        );

        if tester.node().num_connected() == 0 {
            panic!("the fuzz test failed!");
        }
    }
}
