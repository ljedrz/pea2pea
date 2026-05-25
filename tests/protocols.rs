mod common;

use std::{
    collections::{HashMap, HashSet},
    io,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::{Buf, Bytes, BytesMut};
use parking_lot::{Mutex, RwLock};
use pea2pea::{Config, Connection, ConnectionSide, Node, Pea2Pea, protocols::*};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::sleep,
};
use tokio_util::codec::Decoder;
use tracing::*;

use crate::common::{TestCodec, WritingExt, named_node, start_listening, wait_until};

#[tokio::test]
async fn messaging_example() {
    #[derive(PartialEq, Eq, Hash, Debug, Clone, Copy)]
    enum TestMessage {
        Herp,
        Derp,
    }

    impl From<u8> for TestMessage {
        fn from(byte: u8) -> Self {
            match byte {
                0 => Self::Herp,
                1 => Self::Derp,
                _ => panic!("can't deserialize a TestMessage!"),
            }
        }
    }

    impl Decoder for common::TestCodec<TestMessage> {
        type Item = TestMessage;
        type Error = io::Error;

        fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
            Ok(self.0.decode(src)?.map(|mut bytes| bytes.get_u8().into()))
        }
    }

    #[derive(Clone)]
    struct EchoNode {
        node: Node,
        echoed: Arc<Mutex<HashSet<TestMessage>>>,
    }

    impl Pea2Pea for EchoNode {
        fn node(&self) -> &Node {
            &self.node
        }
    }

    impl Reading for EchoNode {
        type Message = TestMessage;
        type Codec = common::TestCodec<Self::Message>;

        fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
            Default::default()
        }

        async fn process_message(&self, source: SocketAddr, message: Self::Message) {
            info!(parent: self.node().span(), "got a {message:?} from {source}");

            if self.echoed.lock().insert(message) {
                info!(parent: self.node().span(), "it was new! echoing it");

                self.send_dm(source, Bytes::copy_from_slice(&[message as u8]))
                    .await
                    .unwrap();
            } else {
                debug!(parent: self.node().span(), "I've already heard {message:?}! not echoing");
            }
        }
    }

    impl Writing for EchoNode {
        type Message = Bytes;
        type Codec = common::TestCodec<Self::Message>;

        fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
            Default::default()
        }
    }

    let shouter = crate::test_node!("shout");
    shouter.enable_reading().await;
    shouter.enable_writing().await;
    start_listening(&shouter).await;

    let picky_echo_config = Config {
        name: Some("picky_echo".into()),
        ..Default::default()
    };
    let picky_echo = EchoNode {
        node: Node::new(picky_echo_config),
        echoed: Default::default(),
    };
    picky_echo.enable_reading().await;
    picky_echo.enable_writing().await;

    let picky_echo_addr = start_listening(&picky_echo).await;

    shouter.node().connect(picky_echo_addr).await.unwrap();

    wait_until(Duration::from_secs(1), || {
        picky_echo.node().num_connected() == 1
    })
    .await;

    for message in &[TestMessage::Herp, TestMessage::Derp, TestMessage::Herp] {
        let msg = Bytes::copy_from_slice(&[*message as u8]);
        shouter.send_dm(picky_echo_addr, msg).await.unwrap();
    }

    // let echo send one message on its own too, for good measure
    let shouter_addr = picky_echo.node().connected_addrs()[0];

    picky_echo
        .send_dm(shouter_addr, [TestMessage::Herp as u8][..].into())
        .await
        .unwrap();

    // check if the shouter heard the (non-duplicate) echoes and the last, non-reply one
    wait_until(Duration::from_secs(1), || {
        shouter.node().stats().received().0 == 3
    })
    .await;
}

#[tokio::test]
async fn drop_connection_on_invalid_message() {
    let reader = crate::test_node!("reader");
    reader.enable_reading().await;
    let reader_addr = start_listening(&reader).await;

    let writer = crate::test_node!("writer");
    writer.enable_writing().await;

    writer.node().connect(reader_addr).await.unwrap();

    wait_until(Duration::from_secs(1), || {
        reader.node().num_connected() == 1
    })
    .await;

    // an invalid message: a zero-length payload
    let bad_message = Bytes::from(vec![]);

    writer.send_dm(reader_addr, bad_message).await.unwrap();

    wait_until(Duration::from_secs(1), || {
        reader.node().num_connected() == 0
    })
    .await;

    // make sure that a disconnected peer is properly cleaned up
    writer.node().disconnect(reader_addr).await;
    let result = writer.unicast_fast(reader_addr, (&b"are you still there?"[..]).into());
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotConnected);
}

#[tokio::test]
async fn drop_connection_on_zero_read() {
    let reader = crate::test_node!("reader");
    reader.enable_reading().await;
    let reader_addr = start_listening(&reader).await;

    let peer = crate::test_node!("peer");

    peer.node().connect(reader_addr).await.unwrap();

    wait_until(Duration::from_secs(1), || {
        reader.node().num_connected() == 1
    })
    .await;

    // the peer shuts down, i.e. disconnects
    peer.node().shut_down().await;

    // the reader should drop its connection too now
    wait_until(Duration::from_secs(1), || {
        reader.node().num_connected() == 0
    })
    .await;
}

#[tokio::test]
async fn no_reading_no_delivery() {
    let reader = crate::test_node!("defunct reader");
    let reader_addr = start_listening(&reader).await;

    let writer = crate::test_node!("writer");
    writer.enable_writing().await;

    writer.node().connect(reader_addr).await.unwrap();

    wait_until(Duration::from_secs(1), || {
        reader.node().num_connected() == 1
    })
    .await;

    // writer sends a message
    writer
        .send_dm(reader_addr, vec![0; 16].into())
        .await
        .unwrap();

    // but the reader didn't enable reading, so it won't receive anything
    wait_until(Duration::from_secs(1), || {
        reader.node().stats().received() == (0, 0)
    })
    .await;
}

#[tokio::test]
async fn no_writing_no_delivery() {
    let reader = crate::test_node!("reader");
    reader.enable_reading().await;
    let reader_addr = start_listening(&reader).await;

    let writer = crate::test_node!("defunct writer");

    writer.node().connect(reader_addr).await.unwrap();

    wait_until(Duration::from_secs(1), || {
        reader.node().num_connected() == 1
    })
    .await;

    // writer tries to send a message
    assert!(
        writer
            .send_dm(reader_addr, vec![0; 16].into())
            .await
            .is_err()
    );

    // the writer didn't enable writing, so the reader won't receive anything
    wait_until(Duration::from_secs(1), || {
        reader.node().stats().received() == (0, 0)
    })
    .await;
}

#[tokio::test]
async fn handshake_guards_connect() {
    #[derive(Clone)]
    struct SimpleHandshakeNode {
        node: Node,
        own_nonce: u64,
        peer_nonces: Arc<RwLock<HashMap<u64, SocketAddr>>>,
    }

    impl SimpleHandshakeNode {
        fn new() -> Self {
            Self {
                node: Node::new(Default::default()),
                own_nonce: rand::random(),
                peer_nonces: Default::default(),
            }
        }

        fn is_nonce_unique(&self, nonce: u64) -> bool {
            self.own_nonce != nonce && !self.peer_nonces.read().contains_key(&nonce)
        }
    }

    impl Pea2Pea for SimpleHandshakeNode {
        fn node(&self) -> &Node {
            &self.node
        }
    }

    impl Handshake for SimpleHandshakeNode {
        const TIMEOUT_MS: u64 = 50;

        async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
            let node_conn_side = !conn.side();
            let stream = self.borrow_stream(&mut conn);

            let peer_nonce = match node_conn_side {
                ConnectionSide::Initiator => {
                    // send own nonce
                    stream.write_u64(self.own_nonce).await.unwrap();

                    // read peer nonce
                    let peer_nonce = stream.read_u64().await.unwrap();

                    // check nonce uniqueness
                    if !self.is_nonce_unique(peer_nonce) {
                        return Err(io::ErrorKind::AlreadyExists.into());
                    }

                    peer_nonce
                }
                ConnectionSide::Responder => {
                    // read peer nonce
                    let peer_nonce = stream.read_u64().await.unwrap();

                    // check nonce uniqueness
                    if !self.is_nonce_unique(peer_nonce) {
                        return Err(io::ErrorKind::AlreadyExists.into());
                    }

                    // send own nonce
                    stream.write_u64(self.own_nonce).await.unwrap();

                    peer_nonce
                }
            };

            // register the handshake nonce
            self.peer_nonces.write().insert(peer_nonce, conn.addr());

            Ok(conn)
        }
    }

    crate::impl_messaging!(SimpleHandshakeNode);

    let initiator = SimpleHandshakeNode::new();
    let responder = SimpleHandshakeNode::new();

    // Reading and Writing are not required for the handshake; they are enabled only so that their relationship
    // with the handshake protocol can be tested too; they should kick in only after the handshake concludes
    for node in [&initiator, &responder] {
        node.enable_reading().await;
        node.enable_writing().await;
        // node.enable_handshake().await;
        start_listening(node).await;
    }

    // the initiator doesn't enable handshakes yet
    responder.enable_handshake().await;

    initiator
        .node()
        .connect(responder.node().listening_addr().await.unwrap())
        .await
        .unwrap();

    // this should fail
    wait_until(Duration::from_millis(500), || {
        initiator.node().num_connecting() == 0
            && responder.node().num_connecting() == 0
            && initiator.node().num_connected() == 0
            && responder.node().num_connected() == 0
    })
    .await;

    // now enable the initiator's handshake logic
    initiator.enable_handshake().await;

    initiator
        .node()
        .connect(responder.node().listening_addr().await.unwrap())
        .await
        .unwrap();

    wait_until(Duration::from_millis(500), || {
        initiator.peer_nonces.read().keys().next() == Some(&responder.own_nonce)
            && responder.peer_nonces.read().keys().next() == Some(&initiator.own_nonce)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn timeout_when_spammed_with_connections() {
    // a wrapper struct with a badly implemented Handshake protocol
    #[derive(Clone)]
    struct BadHandshakeNode(Node);

    impl Pea2Pea for BadHandshakeNode {
        fn node(&self) -> &Node {
            &self.0
        }
    }

    // a badly implemented handshake protocol; 1B is expected by both the initiator and the responder (no distinction
    // is even made), but it is never provided by either of them
    impl Handshake for BadHandshakeNode {
        const TIMEOUT_MS: u64 = 100;

        async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
            let _ = self
                .borrow_stream(&mut conn)
                .read_exact(&mut [0u8; 1])
                .await;

            unreachable!();
        }
    }

    const NUM_ATTEMPTS: u16 = 100;

    let config = Config {
        max_connections: NUM_ATTEMPTS,
        ..Default::default()
    };
    let victim = BadHandshakeNode(Node::new(config));
    victim.enable_handshake().await;
    let victim_addr = start_listening(&victim).await;

    let mut sockets = Vec::with_capacity(NUM_ATTEMPTS as usize);

    for _ in 0..NUM_ATTEMPTS {
        if let Ok(socket) = TcpStream::connect(victim_addr).await {
            sockets.push(socket);
        }
    }

    wait_until(Duration::from_secs(3), || {
        victim.node().num_connecting() == NUM_ATTEMPTS as usize
    })
    .await;

    wait_until(Duration::from_secs(1), || {
        victim.node().num_connecting() + victim.node().num_connected() == 0
    })
    .await;
}

#[tokio::test]
async fn handshake_failure_releases_connecting_slot() {
    #[derive(Clone)]
    struct AlwaysFailHandshakeNode(Node);
    impl Pea2Pea for AlwaysFailHandshakeNode {
        fn node(&self) -> &Node {
            &self.0
        }
    }
    impl Handshake for AlwaysFailHandshakeNode {
        async fn perform_handshake(&self, _conn: Connection) -> io::Result<Connection> {
            Err(io::Error::other("nope"))
        }
    }

    // permissive everything *except* the limit we care about
    let config = Config {
        name: Some("hs_fail".into()),
        max_connecting: 1,
        max_connections: 100,
        ..Default::default()
    };
    let node = AlwaysFailHandshakeNode(Node::new(config));
    node.enable_handshake().await;

    // a fresh target for every attempt so we don't collide with
    // `allow_duplicate_connections == false`
    for _ in 0..3 {
        let target = Node::new(Default::default());
        let addr = target.toggle_listener().await.unwrap().unwrap();

        let err = node.0.connect(addr).await.unwrap_err();
        // The handshake errored - but importantly, the error is NOT
        // QuotaExceeded from the previous attempt failing to release its slot.
        assert_ne!(
            err.kind(),
            io::ErrorKind::QuotaExceeded,
            "connecting slot was not released after a previous handshake failure"
        );
    }

    // and after the dust settles, the counters are back to zero
    wait_until(Duration::from_secs(1), || {
        node.0.num_connecting() == 0 && node.0.num_connected() == 0
    })
    .await;
}

#[tokio::test]
async fn on_connect_message() {
    #[derive(Clone)]
    struct HelloNode(Node);

    impl Pea2Pea for HelloNode {
        fn node(&self) -> &Node {
            &self.0
        }
    }

    impl_messaging!(HelloNode);

    impl OnConnect for HelloNode {
        async fn on_connect(&self, addr: SocketAddr) {
            self.send_dm(addr, "hello!".into()).await.unwrap();
        }
    }

    let connector = HelloNode(named_node("connector"));
    connector.enable_writing().await;
    connector.enable_on_connect().await;

    let connectee = HelloNode(named_node("connectee"));
    connectee.enable_reading().await;
    let connectee_addr = start_listening(&connectee).await;

    connector.node().connect(connectee_addr).await.unwrap();

    wait_until(Duration::from_secs(1), || {
        connectee.node().num_connected() == 1
    })
    .await;

    wait_until(Duration::from_secs(1), || {
        connectee.node().stats().received().0 == 1
    })
    .await;
}

#[tokio::test]
async fn connect_doesnt_wait_for_on_connect_hook() {
    #[derive(Clone)]
    struct SlowNode(Node);

    impl Pea2Pea for SlowNode {
        fn node(&self) -> &Node {
            &self.0
        }
    }

    impl OnConnect for SlowNode {
        async fn on_connect(&self, _addr: SocketAddr) {
            // simulate a slow initialization step (e.g., DB lookup, auth)
            sleep(Duration::from_millis(500)).await;
        }
    }

    let initiator = SlowNode(Node::new(Default::default()));
    initiator.enable_on_connect().await;

    let target = crate::test_node!("target");
    let target_addr = start_listening(&target).await;

    let start = Instant::now();

    // this should not return until the 500ms sleep in on_connect finishes
    initiator.node().connect(target_addr).await.unwrap();

    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_millis(500),
        "node.connect() returned too late!"
    );
    assert_eq!(initiator.node().num_connected(), 1);
}

#[tokio::test]
async fn on_connect_non_abortable_runs_to_completion() {
    #[derive(Clone)]
    struct StubbornNode {
        node: Node,
        completed: Arc<AtomicBool>,
    }
    impl Pea2Pea for StubbornNode {
        fn node(&self) -> &Node {
            &self.node
        }
    }
    impl OnConnect for StubbornNode {
        const ABORTABLE: bool = false;
        async fn on_connect(&self, _: SocketAddr) {
            // give the test enough time to issue a disconnect *before* this
            // body finishes
            sleep(Duration::from_millis(300)).await;
            self.completed.store(true, Ordering::Release);
        }
    }

    let stubborn = StubbornNode {
        node: Node::new(Default::default()),
        completed: Arc::new(AtomicBool::new(false)),
    };
    stubborn.enable_on_connect().await;

    let target = crate::test_node!("target");
    let target_addr = start_listening(&target).await;

    stubborn.node().connect(target_addr).await.unwrap();

    // disconnect before the OnConnect body finishes
    stubborn.node().disconnect(target_addr).await;

    // despite the disconnect, the non-abortable hook must still run
    wait_until(Duration::from_secs(2), || {
        stubborn.completed.load(Ordering::Acquire)
    })
    .await;
}

/// The default (`ABORTABLE = true`) means the OnConnect task is attached
/// to the connection and is aborted when the connection is dropped.
#[tokio::test]
async fn on_connect_abortable_is_cancelled_on_disconnect() {
    #[derive(Clone)]
    struct AbortableNode {
        node: Node,
        completed: Arc<AtomicBool>,
    }
    impl Pea2Pea for AbortableNode {
        fn node(&self) -> &Node {
            &self.node
        }
    }
    impl OnConnect for AbortableNode {
        // ABORTABLE: bool = true is the default
        async fn on_connect(&self, _: SocketAddr) {
            // long enough that we can disconnect well before this finishes
            sleep(Duration::from_secs(5)).await;
            self.completed.store(true, Ordering::Release);
        }
    }

    let abortable = AbortableNode {
        node: Node::new(Default::default()),
        completed: Arc::new(AtomicBool::new(false)),
    };
    abortable.enable_on_connect().await;

    let target = crate::test_node!("target");
    let target_addr = start_listening(&target).await;

    abortable.node().connect(target_addr).await.unwrap();
    abortable.node().disconnect(target_addr).await;

    // wait substantially less than the OnConnect body would take if it ran -
    // the body was aborted, so `completed` must still be false.
    sleep(Duration::from_millis(300)).await;
    assert!(
        !abortable.completed.load(Ordering::Acquire),
        "abortable OnConnect should have been cancelled on disconnect"
    );
}

#[tokio::test]
async fn on_disconnect_timeout_aborts_slow_hook() {
    #[derive(Clone)]
    struct SlowDisconnectNode(Node);
    impl Pea2Pea for SlowDisconnectNode {
        fn node(&self) -> &Node {
            &self.0
        }
    }
    impl OnDisconnect for SlowDisconnectNode {
        const TIMEOUT_MS: u64 = 200;
        async fn on_disconnect(&self, _: SocketAddr) {
            sleep(Duration::from_secs(10)).await;
        }
    }

    let slow = SlowDisconnectNode(Node::new(Default::default()));
    slow.enable_on_disconnect().await;
    let slow_addr = slow.0.toggle_listener().await.unwrap().unwrap();

    let peer = crate::test_node!("peer");
    peer.node().connect(slow_addr).await.unwrap();

    wait_until(Duration::from_secs(1), || slow.0.num_connected() == 1).await;

    let peer_addr = slow.0.connected_addrs()[0];

    // `disconnect` waits on the hook, but only up to TIMEOUT_MS
    let start = Instant::now();
    assert!(slow.0.disconnect(peer_addr).await);
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "OnDisconnect timeout did not fire: disconnect took {elapsed:?}"
    );
}

#[tokio::test]
async fn on_disconnect_fires_on_both_sides() {
    #[derive(Clone)]
    struct TrackingNode {
        node: Node,
        fired: Arc<AtomicBool>,
    }

    impl TrackingNode {
        fn new(name: &str, fired: Arc<AtomicBool>) -> Self {
            Self {
                node: Node::new(Config {
                    name: Some(name.into()),
                    ..Default::default()
                }),
                fired,
            }
        }
    }

    impl Pea2Pea for TrackingNode {
        fn node(&self) -> &Node {
            &self.node
        }
    }

    impl OnDisconnect for TrackingNode {
        async fn on_disconnect(&self, _: SocketAddr) {
            self.fired.store(true, Ordering::Release);
        }
    }

    // No-op Reading: needed on the passive side so it notices the peer's FIN
    // and runs the disconnect cleanup path.
    impl Reading for TrackingNode {
        type Message = BytesMut;
        type Codec = TestCodec<BytesMut>;

        fn codec(&self, _: SocketAddr, _: ConnectionSide) -> Self::Codec {
            Default::default()
        }

        async fn process_message(&self, _: SocketAddr, _: Self::Message) {}
    }

    let a_fired = Arc::new(AtomicBool::new(false));
    let b_fired = Arc::new(AtomicBool::new(false));

    let a = TrackingNode::new("a", a_fired.clone());
    let b = TrackingNode::new("b", b_fired.clone());

    // `a` calls disconnect(), so it knows immediately; `b` needs Reading
    // enabled to detect the FIN and run its OnDisconnect.
    b.enable_reading().await;
    a.enable_on_disconnect().await;
    b.enable_on_disconnect().await;

    let b_addr = start_listening(&b).await;
    a.node().connect(b_addr).await.unwrap();
    wait_until(Duration::from_secs(1), || b.node().num_connected() == 1).await;

    a.node().disconnect(b_addr).await;

    wait_until(Duration::from_secs(1), || {
        a_fired.load(Ordering::Acquire) && b_fired.load(Ordering::Acquire)
    })
    .await;
}
