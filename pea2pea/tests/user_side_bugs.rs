mod common;

use std::{io, net::SocketAddr, sync::Arc, time::Duration};

use bytes::BytesMut;
use pea2pea::{
    Config, Connection, ConnectionSide, Node, Pea2Pea, connections::DisconnectOrigin, protocols::*,
};
use tokio::{io::AsyncWriteExt, net::TcpStream, sync::Notify, time::timeout};
use tokio_util::codec::{BytesCodec, Decoder, Encoder};

use crate::common::{start_listening, wait_for_connections};

macro_rules! define_fail_node {
    () => {
        #[derive(Clone)]
        struct FailNode {
            node: Node,
            notify: Arc<Notify>,
        }

        impl FailNode {
            fn new_with_notifier() -> (Self, Arc<Notify>) {
                let notify = Arc::new(Notify::new());
                let node = FailNode {
                    node: Node::new(Config::default()),
                    notify: notify.clone(),
                };

                (node, notify)
            }
        }

        impl Pea2Pea for FailNode {
            fn node(&self) -> &Node {
                &self.node
            }
        }
    };
}

fn silence_panics() {
    std::panic::set_hook(Box::new(|_| {}));
}

async fn timed_notified(notify: &Notify) {
    timeout(Duration::from_secs(1), notify.notified())
        .await
        .expect("notify timeout - node likely died after a panic");
}

#[tokio::test]
async fn broken_handshake_impl() {
    silence_panics();
    define_fail_node!();

    impl Handshake for FailNode {
        async fn perform_handshake(&self, _conn: Connection) -> io::Result<Connection> {
            self.notify.notify_one();
            panic!("PARKOUR!");
        }
    }

    let (fail_node, notify) = FailNode::new_with_notifier();
    fail_node.enable_handshake().await;
    let addr = start_listening(&fail_node).await;

    for _ in 0..2 {
        // this shouldn't blow up neither the fail_node, nor the test
        let _ = TcpStream::connect(addr).await.unwrap();
        // this both proves that the handshake impl has fired, and tells us when to proceed
        timed_notified(&notify).await;
    }
}

#[tokio::test]
async fn broken_reading_codec() {
    silence_panics();
    define_fail_node!();

    struct FailCodec(Arc<Notify>);

    impl Decoder for FailCodec {
        type Item = ();
        type Error = io::Error;

        fn decode(&mut self, _src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
            self.0.notify_one();
            panic!("HOLD MY BEER!");
        }
    }

    impl Reading for FailNode {
        type Message = (); // unimportant for this test
        type Codec = FailCodec;

        fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
            FailCodec(self.notify.clone())
        }

        async fn process_message(&self, _source: SocketAddr, _message: Self::Message) {
            // we'll break this in another test, it wouldn't make a difference here
        }
    }

    let (fail_node, notify) = FailNode::new_with_notifier();
    fail_node.enable_reading().await;
    let addr = start_listening(&fail_node).await;

    for _ in 0..2 {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        // this shouldn't blow up neither the fail_node, nor the test
        stream.write_u8(0).await.unwrap();
        stream.flush().await.unwrap();
        // this both proves that the codec was created, and tells us when to proceed
        timed_notified(&notify).await;
    }
}

#[tokio::test]
async fn broken_reading_impl() {
    silence_panics();
    define_fail_node!();

    impl Reading for FailNode {
        type Message = BytesMut;
        type Codec = BytesCodec;

        fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
            Default::default()
        }

        async fn process_message(&self, _source: SocketAddr, _message: Self::Message) {
            self.notify.notify_one();
            panic!("LEEROOOOY... JEENKIINS!");
        }
    }

    let (fail_node, notify) = FailNode::new_with_notifier();
    fail_node.enable_reading().await;
    let addr = start_listening(&fail_node).await;

    for _ in 0..2 {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        // this shouldn't blow up neither the fail_node, nor the test
        stream.write_u8(0).await.unwrap();
        stream.flush().await.unwrap();
        // this both proves that the message was read, and tells us when to proceed
        timed_notified(&notify).await;
    }
}

#[tokio::test]
async fn broken_writing_impl() {
    silence_panics();
    define_fail_node!();

    struct FailCodec(Arc<Notify>);

    impl Encoder<()> for FailCodec {
        type Error = io::Error;

        fn encode(&mut self, _item: (), _dst: &mut BytesMut) -> Result<(), Self::Error> {
            self.0.notify_one();
            panic!("All those moments will be lost in time, like tears in rain. Time to die.");
        }
    }

    impl Writing for FailNode {
        type Message = ();
        type Codec = FailCodec;

        fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
            FailCodec(self.notify.clone())
        }
    }

    let (fail_node, notify) = FailNode::new_with_notifier();
    fail_node.enable_writing().await;
    let addr = start_listening(&fail_node).await;

    for _ in 0..2 {
        let _stream = TcpStream::connect(addr).await.unwrap();
        wait_for_connections(fail_node.node(), 1).await;
        let stream_addr = fail_node.node().connected_addrs()[0];
        // this shouldn't blow up neither the fail_node, nor the test
        let _ = fail_node.unicast(stream_addr, ()).unwrap().await;
        // this both proves that the codec was created, and tells us when to proceed
        timed_notified(&notify).await;
        // without reading or writing, the failed node won't auto-disconnect quickly
        fail_node.node().disconnect(stream_addr).await;
    }
}

#[tokio::test]
async fn broken_on_connect_impl() {
    silence_panics();
    define_fail_node!();

    impl OnConnect for FailNode {
        async fn on_connect(&self, _: SocketAddr) {
            self.notify.notify_one();
            panic!("This is Sparta!");
        }
    }

    let (fail_node, notify) = FailNode::new_with_notifier();
    fail_node.enable_on_connect().await;
    let addr = start_listening(&fail_node).await;

    for _ in 0..2 {
        let _ = TcpStream::connect(addr).await.unwrap();
        // this both proves that the hook was triggered, and tells us when to proceed
        timed_notified(&notify).await;
    }
}

#[tokio::test]
async fn broken_on_disconnect_impl() {
    silence_panics();
    define_fail_node!();

    impl OnDisconnect for FailNode {
        async fn on_disconnect(&self, _: SocketAddr, _: DisconnectOrigin) {
            self.notify.notify_one();
            panic!("Genug.");
        }
    }

    let (fail_node, notify) = FailNode::new_with_notifier();
    fail_node.enable_on_disconnect().await;
    let addr = start_listening(&fail_node).await;

    for _ in 0..2 {
        let _stream = TcpStream::connect(addr).await.unwrap();
        wait_for_connections(fail_node.node(), 1).await;
        // without reading or writing, the failed node won't auto-disconnect quickly
        let stream_addr = fail_node.node().connected_addrs()[0];
        fail_node.node().disconnect(stream_addr).await;
        // this both proves that the hook was triggered, and tells us when to proceed
        timed_notified(&notify).await;
    }
}
