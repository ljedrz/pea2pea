#![no_main]

//! This fuzz test is designed primarily to check the node's inbound message buffering/handling setup, as the
//! library doesn't provide any default means to (de)serialize messages. It's somewhat tricky to test this for
//! any setup, as `Reading::read_message` takes an active role in buffering and it must have _some_ impl in
//! order for the protocol to work; we're using a very simple protocol with length-prefixed messages where
//! their payload is not deserialized at all in order to test the library itself (as opposed to any specific
//! implementation) as much as possible.
//!
//! Feel free to reuse this code to test your own implementation of `Pea2Pea` protocols.

use libfuzzer_sys::fuzz_target;
use pea2pea::{
    protocols::{Reading, Writing},
    Config, Node, Pea2Pea,
};
use tokio::time::sleep;

use std::{
    convert::TryInto,
    io::{self, ErrorKind, Read, Write},
    net::SocketAddr,
    time::Duration,
};

#[derive(Clone)]
pub struct FuzzNode(pub Node);

impl Pea2Pea for FuzzNode {
    fn node(&self) -> &Node {
        &self.0
    }
}

// no need for a large size, since we're only testing the buffering setup; this value should be
// equal to or greater than the `max_len` argument passed to `cargo-fuzz`, as the `Reading` impl
// will just reject any larger messages
const MAX_MSG_SIZE: usize = 256;

#[async_trait::async_trait]
impl Reading for FuzzNode {
    type Message = Vec<u8>;

    fn read_message<R: Read>(
        &self,
        _source: SocketAddr,
        reader: &mut R,
    ) -> io::Result<Option<Self::Message>> {
        // expect a prefix with a u16 LE length of the actual message
        let mut len_arr = [0u8; 2];
        if reader.read_exact(&mut len_arr).is_err() {
            return Ok(None);
        }
        let payload_len = u16::from_le_bytes(len_arr[..].try_into().unwrap()) as usize;

        // a zero-length payload would normally be treated as an error and possibly trigger
        // a disconnect, but we'll instead accept those and theat them as empty inbound
        // messages in order to have as much coverage as possible
        if payload_len == 0 {
            return Ok(Some(vec![]));
        }

        // we are deliberately using an `ErrorKind` that does not belong to `Config::fatal_io_errors`
        // in order not to cause a disconnect, which would end the fuzz test prematurely; the `Reading`
        // protocol already enforces a size limit internally (via `Config::read_buffer_size`), but
        // performing this check here avoids the aforementioned issue
        if payload_len > MAX_MSG_SIZE {
            return Err(ErrorKind::Other.into());
        }

        // this can be done more efficiently by having persistent per-connection buffers, but it's
        // perfectly good enough for test purposes
        let mut buffer = vec![0u8; payload_len];
        if reader
            .take(payload_len as u64)
            .read_exact(&mut buffer)
            .is_err()
        {
            Ok(None)
        } else {
            Ok(Some(buffer))
        }
    }

    async fn process_message(
        &self,
        _source: SocketAddr,
        _message: Self::Message,
    ) -> io::Result<()> {
        Ok(())
    }
}

impl Writing for FuzzNode {
    type Message = Vec<u8>;

    fn write_message<W: Write>(
        &self,
        _target: SocketAddr,
        payload: &Self::Message,
        writer: &mut W,
    ) -> io::Result<()> {
        // prefix the actual message with its length encoded as a u16 LE
        writer.write_all(&(payload.len() as u16).to_le_bytes())?;
        writer.write_all(payload)
    }
}

// the fuzzing will be done using `Vec<u8>` messages
fuzz_target!(|messages: Vec<Vec<u8>>| {
    let rt = tokio::runtime::Runtime::new().unwrap();

    // this value could just be equal to `MAX_MSG_SIZE`, but it's greater
    // for a potential performance boost
    let config = Config {
        read_buffer_size: MAX_MSG_SIZE * 2,
        ..Default::default()
    };

    rt.block_on(async {
        let sender = FuzzNode(Node::new(None).await.unwrap());
        let receiver = FuzzNode(Node::new(Some(config)).await.unwrap());

        receiver.enable_reading().await;
        sender.enable_writing().await;

        let receiver_addr = receiver.node().listening_addr().unwrap();
        sender.node().connect(receiver_addr).await.unwrap();

        // a small delay to ensure that both nodes are fully connected
        sleep(Duration::from_millis(10)).await;

        for msg in messages {
            // we await the `oneshot::Receiver` returned by `Writing::send_direct_message` every time
            // in order to not "clog" the inbound queue of the `receiver` node
            assert!(sender
                .send_direct_message(receiver_addr, msg.into())
                .unwrap()
                .await
                .unwrap());
        }
    });
});
