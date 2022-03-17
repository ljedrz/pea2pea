#![no_main]

//! This fuzz test is designed primarily to check the node's inbound message buffering/handling setup, as the
//! library doesn't provide any default means to (de)serialize messages. It's somewhat tricky to test this for
//! any setup, as `Reading::read_message` takes an active role in buffering and it must have _some_ impl in
//! order for the protocol to work; we're using a very simple protocol with length-prefixed messages where
//! their payload is not deserialized at all in order to test the library itself (as opposed to any specific
//! implementation) as much as possible.
//!
//! Feel free to reuse this code to test your own implementation of `Pea2Pea` protocols.

use bytes::{Bytes, BytesMut};
use libfuzzer_sys::fuzz_target;
use pea2pea::{
    protocols::{Reading, Writing},
    Node, Pea2Pea,
};
use tokio::time::sleep;
use tokio_util::codec::LengthDelimitedCodec;

use std::{io, net::SocketAddr, time::Duration};

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
    type Message = BytesMut;
    type Codec = LengthDelimitedCodec;

    fn codec(&self, _addr: SocketAddr) -> Self::Codec {
        LengthDelimitedCodec::builder()
            .max_frame_length(MAX_MSG_SIZE)
            .length_field_length(2)
            .new_codec()
    }

    async fn process_message(&self, _src: SocketAddr, _msg: Self::Message) -> io::Result<()> {
        Ok(())
    }
}

impl Writing for FuzzNode {
    type Message = Bytes;
    type Codec = LengthDelimitedCodec;

    fn codec(&self, _addr: SocketAddr) -> Self::Codec {
        LengthDelimitedCodec::builder()
            .max_frame_length(MAX_MSG_SIZE)
            .length_field_length(2)
            .new_codec()
    }
}

// the fuzzing will be done using `Vec<u8>` messages
fuzz_target!(|messages: Vec<Vec<u8>>| {
    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(async {
        let sender = FuzzNode(Node::new(None).await.unwrap());
        let receiver = FuzzNode(Node::new(None).await.unwrap());

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
