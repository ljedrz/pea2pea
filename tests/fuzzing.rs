use rand::{distributions::Standard, rngs::SmallRng, Rng, SeedableRng};
use tokio::time::sleep;

use pea2pea::{ContainsNode, Messaging, Node, NodeConfig};

use std::{convert::TryInto, io, sync::Arc, time::Duration};

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
        // expecting the test messages to be prefixed with their length encoded as a LE u32
        if buffer.len() >= 4 {
            let payload_len = u32::from_le_bytes(buffer[..4].try_into().unwrap()) as usize;

            if payload_len == 0 {
                return Err(io::ErrorKind::InvalidData.into());
            }

            if buffer[4..].len() >= payload_len {
                Ok(Some(&buffer[..4 + payload_len]))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
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
    sleep(Duration::from_millis(10)).await;

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
