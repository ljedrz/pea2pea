use rand::{distributions::Standard, rngs::SmallRng, Rng, SeedableRng};

mod common;
use pea2pea::{ContainsNode, Messaging, Node, NodeConfig};

use std::{io, sync::Arc};

#[derive(Clone)]
struct Tester(Arc<Node>);

impl ContainsNode for Tester {
    fn node(&self) -> &Arc<Node> {
        &self.0
    }
}

#[async_trait::async_trait]
impl Messaging for Tester {
    fn read_message(buffer: &[u8]) -> io::Result<Option<&[u8]>> {
        common::read_len_prefixed_message(4, buffer)
    }
}

#[ignore]
#[tokio::test]
async fn fuzzing() {
    const MAX_MSG_SIZE: usize = 256;

    let mut config = NodeConfig::default();
    config.conn_read_buffer_size = MAX_MSG_SIZE;
    let tester = Tester(Node::new(Some(config)).await.unwrap());
    tester.enable_messaging();

    let sender = Node::new(None).await.unwrap();

    sender
        .initiate_connection(tester.node().listening_addr)
        .await
        .unwrap();

    wait_until!(1, tester.node().num_handshaken() == 1);

    let mut rng = SmallRng::from_entropy();

    loop {
        let random_len: usize = rng.gen_range(5..MAX_MSG_SIZE); // account for the length prefix
        let mut random_message: Vec<u8> =
            (&mut rng).sample_iter(Standard).take(random_len).collect();
        random_message[0..4].copy_from_slice(&(random_len as u32 - 4).to_le_bytes());
        sender
            .send_direct_message(tester.node().listening_addr, random_message.into())
            .await
            .unwrap();

        if tester.node().num_connected() == 0 {
            panic!("the fuzz test failed!");
        }
    }
}
