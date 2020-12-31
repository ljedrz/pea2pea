use rand::{distributions::Standard, rngs::SmallRng, Rng, SeedableRng};

mod common;
use pea2pea::{
    protocols::{Reading, Writing},
    Node, NodeConfig, Pea2Pea,
};

use std::{io, net::SocketAddr, sync::Arc};

#[derive(Clone)]
struct Tester(Arc<Node>);

impl Pea2Pea for Tester {
    fn node(&self) -> &Arc<Node> {
        &self.0
    }
}

#[async_trait::async_trait]
impl Reading for Tester {
    type Message = ();

    fn read_message(
        &self,
        _source: SocketAddr,
        buffer: &[u8],
    ) -> io::Result<Option<(Self::Message, usize)>> {
        let bytes = common::read_len_prefixed_message(2, buffer)?;

        Ok(bytes.map(|bytes| ((), bytes.len())))
    }
}

#[ignore]
#[tokio::test]
async fn fuzzing() {
    const MAX_MSG_SIZE: usize = 256;

    let mut config = NodeConfig::default();
    config.conn_read_buffer_size = MAX_MSG_SIZE;
    let tester = Tester(Node::new(Some(config)).await.unwrap());
    tester.enable_reading();

    let sender = common::MessagingNode::new("sender").await;
    sender.enable_writing();

    sender
        .node()
        .initiate_connection(tester.node().listening_addr)
        .await
        .unwrap();

    wait_until!(1, tester.node().num_connected() == 1);

    let mut rng = SmallRng::from_entropy();

    loop {
        let random_len: usize = rng.gen_range(1..MAX_MSG_SIZE - 2); // account for the length prefix
        let random_payload: Vec<u8> = (&mut rng).sample_iter(Standard).take(random_len).collect();
        sender
            .node()
            .send_direct_message(tester.node().listening_addr, random_payload.into())
            .await
            .unwrap();

        if tester.node().num_connected() == 0 {
            panic!("the fuzz test failed!");
        }
    }
}
