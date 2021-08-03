use rand::{distributions::Standard, rngs::SmallRng, Rng, SeedableRng};

mod common;
use pea2pea::{
    protocols::{Reading, Writing},
    Node, NodeConfig, Pea2Pea,
};

use std::{io, net::SocketAddr};

#[derive(Clone)]
struct Tester(Node);

impl Pea2Pea for Tester {
    fn node(&self) -> &Node {
        &self.0
    }
}

impl Reading for Tester {
    type Message = ();

    fn read_message(
        &self,
        _source: SocketAddr,
        buffer: &[u8],
    ) -> io::Result<Option<(Self::Message, usize)>> {
        let bytes = common::read_len_prefixed_message(4, buffer)?;

        Ok(bytes.map(|bytes| ((), bytes.len())))
    }
}

impl Writing for Tester {
    fn write_message(
        &self,
        _target: SocketAddr,
        payload: &[u8],
        buffer: &mut [u8],
    ) -> io::Result<usize> {
        buffer[..4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        buffer[4..][..payload.len()].copy_from_slice(payload);
        Ok(4 + payload.len())
    }
}

#[ignore]
#[tokio::test]
async fn fuzzing() {
    const MAX_MSG_SIZE: usize = 1024 * 1024;

    let config = NodeConfig {
        conn_read_buffer_size: MAX_MSG_SIZE,
        listener_ip: "127.0.0.1".parse().unwrap(),
        ..Default::default()
    };
    let tester = Tester(Node::new(Some(config)).await.unwrap());
    tester.enable_reading();

    let config = NodeConfig {
        conn_write_buffer_size: MAX_MSG_SIZE,
        listener_ip: "127.0.0.1".parse().unwrap(),
        ..Default::default()
    };
    let sender = Tester(Node::new(Some(config)).await.unwrap());
    sender.enable_writing();

    sender
        .node()
        .connect(tester.node().listening_addr())
        .await
        .unwrap();

    wait_until!(1, tester.node().num_connected() == 1);

    let mut rng = SmallRng::from_entropy();

    loop {
        let random_len: usize = rng.gen_range(1..MAX_MSG_SIZE - 2); // account for the length prefix
        let random_payload: Vec<u8> = (&mut rng).sample_iter(Standard).take(random_len).collect();
        sender
            .node()
            .send_direct_message(tester.node().listening_addr(), random_payload.into())
            .unwrap();

        if tester.node().num_connected() == 0 {
            panic!("the fuzz test failed!");
        }
    }
}
