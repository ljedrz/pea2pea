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
use pea2pea::{
    Config, Connection, ConnectionSide, Node, Pea2Pea, connections::DisconnectOrigin, protocols::*,
};
use test_utils::{WritingExt, start_listening, wait_for_connections, wait_until};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::{sleep, timeout},
};
use tokio_util::codec::Decoder;
use tracing::*;

mod common;
use crate::common::{TestCodec, TestNode};

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

    let shouter = TestNode::default();
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

    wait_for_connections(picky_echo.node(), 1).await;

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
    let reader = TestNode::default();
    reader.enable_reading().await;
    let reader_addr = start_listening(&reader).await;

    let writer = TestNode::default();
    writer.enable_writing().await;

    writer.node().connect(reader_addr).await.unwrap();

    wait_for_connections(reader.node(), 1).await;

    // an invalid message: a zero-length payload
    let bad_message = Bytes::from(vec![]);

    writer.send_dm(reader_addr, bad_message).await.unwrap();

    wait_for_connections(reader.node(), 0).await;

    // make sure that a disconnected peer is properly cleaned up
    writer.node().disconnect(reader_addr).await;
    let result = writer.unicast_fast(reader_addr, (&b"are you still there?"[..]).into());
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotConnected);
}

#[tokio::test]
async fn drop_connection_on_zero_read() {
    let reader = TestNode::default();
    reader.enable_reading().await;
    let reader_addr = start_listening(&reader).await;

    let peer = TestNode::default();

    peer.node().connect(reader_addr).await.unwrap();

    wait_for_connections(reader.node(), 1).await;

    // the peer shuts down, i.e. disconnects
    peer.node().shut_down().await;

    // the reader should drop its connection too now
    wait_for_connections(reader.node(), 0).await;
}

#[tokio::test]
async fn no_reading_no_delivery() {
    let reader = TestNode::default();
    let reader_addr = start_listening(&reader).await;

    let writer = TestNode::default();
    writer.enable_writing().await;

    writer.node().connect(reader_addr).await.unwrap();

    wait_for_connections(reader.node(), 1).await;

    // writer sends a message
    writer
        .send_dm(reader_addr, vec![0; 16].into())
        .await
        .unwrap();

    // but the reader didn't enable reading, so it won't receive anything
    assert_eq!(reader.node().stats().received(), (0, 0));
}

#[tokio::test]
async fn no_writing_no_delivery() {
    let reader = TestNode::default();
    reader.enable_reading().await;
    let reader_addr = start_listening(&reader).await;

    let writer = TestNode::default();

    writer.node().connect(reader_addr).await.unwrap();

    wait_for_connections(reader.node(), 1).await;

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

    impl_messaging!(SimpleHandshakeNode);

    let initiator = SimpleHandshakeNode::new();
    let responder = SimpleHandshakeNode::new();

    // Reading and Writing are not required for the handshake; they are enabled only so that their relationship
    // with the handshake protocol can be tested too; they should kick in only after the handshake concludes
    for node in [&initiator, &responder] {
        node.enable_reading().await;
        node.enable_writing().await;
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

#[tokio::test]
async fn failed_handshake_blocks_reading_and_writing() {
    #[derive(Clone)]
    struct GatekeeperNode {
        node: Node,
        // set only if Reading ever delivers a message
        read_fired: Arc<AtomicBool>,
    }

    impl Pea2Pea for GatekeeperNode {
        fn node(&self) -> &Node {
            &self.node
        }
    }

    // the handshake always rejects the connection
    impl Handshake for GatekeeperNode {
        async fn perform_handshake(&self, _conn: Connection) -> io::Result<Connection> {
            Err(io::Error::other("denied"))
        }
    }

    // Reading + Writing are both enabled, but must stay dormant until the
    // handshake concludes successfully - which it never does here.
    impl Reading for GatekeeperNode {
        type Message = BytesMut;
        type Codec = TestCodec<BytesMut>;

        fn codec(&self, _: SocketAddr, _: ConnectionSide) -> Self::Codec {
            Default::default()
        }

        async fn process_message(&self, _: SocketAddr, _: Self::Message) {
            // reaching this means Reading engaged on a connection that never
            // completed its handshake - the invariant is already broken
            self.read_fired.store(true, Ordering::Release);
        }
    }

    impl Writing for GatekeeperNode {
        type Message = Bytes;
        type Codec = TestCodec<Bytes>;

        fn codec(&self, _: SocketAddr, _: ConnectionSide) -> Self::Codec {
            Default::default()
        }
    }

    let gatekeeper = GatekeeperNode {
        node: Node::new(Config {
            name: Some("gatekeeper".into()),
            ..Default::default()
        }),
        read_fired: Default::default(),
    };
    gatekeeper.enable_handshake().await;
    gatekeeper.enable_reading().await;
    gatekeeper.enable_writing().await;
    let addr = start_listening(&gatekeeper).await;

    // a raw client completes the TCP connect, then pushes a perfectly valid
    // frame (TestCodec = 2-byte BE length prefix + payload). The frame being
    // *valid* is deliberate: it rules out "the codec just rejected garbage" as
    // the reason nothing was processed. Writes are best-effort - if the
    // gatekeeper resets us first, that only reinforces the point.
    let mut client = TcpStream::connect(addr).await.unwrap();
    let _ = client
        .write_all(&[0x00, 0x05, b'h', b'e', b'l', b'l', b'o'])
        .await;
    let _ = client.flush().await;

    // the handshake rejected it: the connection must never reach "connected",
    // and the connecting slot must be released
    wait_until(Duration::from_secs(1), || {
        gatekeeper.node().num_connected() == 0 && gatekeeper.node().num_connecting() == 0
    })
    .await;

    // Reading never ran: no message delivered, and node-level receive stats
    // stayed empty despite a valid frame sitting in the socket.
    assert!(
        !gatekeeper.read_fired.load(Ordering::Acquire),
        "Reading delivered a message on a connection that failed its handshake"
    );
    assert_eq!(
        gatekeeper.node().stats().received(),
        (0, 0),
        "Reading engaged"
    );

    // Writing never ran: nothing was ever sent, and there's no peer to send to.
    assert_eq!(gatekeeper.node().stats().sent(), (0, 0), "Writing engaged");
    assert!(gatekeeper.node().connected_addrs().is_empty());

    // ...and from the peer's side, the socket was actually closed rather than
    // left half-open. read() returning (rather than blocking) is the real check
    // here; the timeout guards against a "rejected but never hung up" regression.
    let mut buf = [0u8; 16];
    let n = tokio::time::timeout(Duration::from_secs(1), client.read(&mut buf))
        .await
        .expect("client read timed out - the socket should have been closed")
        .unwrap_or(0); // a read error (reset) is also "connection gone"
    assert_eq!(
        n, 0,
        "got bytes back from a connection that never handshook"
    );
}

#[tokio::test]
async fn successful_handshake_releases_connecting_slot() {
    #[derive(Clone)]
    struct NoopHandshakeNode(Node);
    impl Pea2Pea for NoopHandshakeNode {
        fn node(&self) -> &Node {
            &self.0
        }
    }
    impl Handshake for NoopHandshakeNode {
        async fn perform_handshake(&self, conn: Connection) -> io::Result<Connection> {
            Ok(conn)
        }
    }

    // max_connecting=1 makes slot-leak bugs immediately visible: if the slot
    // isn't released after a successful handshake, the next connect would
    // fail with QuotaExceeded.
    let config = Config {
        name: Some("hs_ok".into()),
        max_connecting: 1,
        max_connections: 100,
        ..Default::default()
    };
    let node = NoopHandshakeNode(Node::new(config));
    node.enable_handshake().await;

    // a fresh target for every iteration, since allow_duplicate_connections is false
    for _ in 0..3 {
        let target = Node::new(Default::default());
        let addr = target.toggle_listener().await.unwrap().unwrap();

        node.0.connect(addr).await.unwrap();
        // by the time connect() returns Ok, the connecting slot must be released
        assert_eq!(node.0.num_connecting(), 0);
    }

    // all three connections are still up
    assert_eq!(node.0.num_connected(), 3);
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

    // every stalled handshake that reached the node timed out, and each is recorded as such
    wait_until(Duration::from_secs(1), || {
        victim.node().heuristics().handshake_timeouts() == NUM_ATTEMPTS as u64
    })
    .await;
}

#[tokio::test]
async fn idle_timeout_is_recorded_in_heuristics() {
    // a reader-enabled node that considers a connection dead after a short idle period
    #[derive(Clone)]
    struct IdleNode(Node);
    impl Pea2Pea for IdleNode {
        fn node(&self) -> &Node {
            &self.0
        }
    }
    impl Reading for IdleNode {
        type Message = BytesMut;
        type Codec = TestCodec<BytesMut>;
        const IDLE_TIMEOUT_MS: u64 = 100;

        fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
            Default::default()
        }

        async fn process_message(&self, _source: SocketAddr, _message: Self::Message) {}
    }

    let node = IdleNode(Node::new(Default::default()));
    node.enable_reading().await;
    let addr = start_listening(&node).await;

    // a bare peer that connects and then stays silent, never sending a frame
    let _silent = TcpStream::connect(addr).await.unwrap();

    // the reader admits the connection, waits IDLE_TIMEOUT_MS for a frame, then drops it as idle
    wait_until(Duration::from_secs(2), || {
        node.node().heuristics().idle_timeouts() == 1
    })
    .await;

    assert_eq!(node.node().num_connected(), 0);
    // an idle timeout must not be miscounted as a handshake or write timeout
    assert_eq!(node.node().heuristics().handshake_timeouts(), 0);
    assert_eq!(node.node().heuristics().write_timeouts(), 0);
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

    let connector = HelloNode(Node::new(Config::default()));
    connector.enable_writing().await;
    connector.enable_on_connect().await;

    let connectee = HelloNode(Node::new(Config::default()));
    connectee.enable_reading().await;
    let connectee_addr = start_listening(&connectee).await;

    connector.node().connect(connectee_addr).await.unwrap();

    wait_for_connections(connectee.node(), 1).await;

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

    let target = TestNode::default();
    let target_addr = start_listening(&target).await;

    let start = Instant::now();

    // this should return as soon as the connection is ready,
    // and not wait for OnConnect logic to conclude
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

    let target = TestNode::default();
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

    let target = TestNode::default();
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
async fn cancelled_connect_keeps_on_connect_abortable() {
    use std::{future::poll_fn, task::Poll};

    #[derive(Clone)]
    struct HookNode {
        node: Node,
        started: Arc<AtomicBool>,
        completed: Arc<AtomicBool>,
    }
    impl Pea2Pea for HookNode {
        fn node(&self) -> &Node {
            &self.node
        }
    }
    impl OnConnect for HookNode {
        // ABORTABLE: bool = true is the default
        async fn on_connect(&self, _: SocketAddr) {
            self.started.store(true, Ordering::Release);
            // shorter than the post-disconnect observation window below: a hook that escaped
            // the disconnect's abort (i.e. one detached by the cancelled `connect`) completes
            // and flips `completed` well before the final assertion samples it
            sleep(Duration::from_millis(500)).await;
            self.completed.store(true, Ordering::Release);
        }
    }

    let initiator = HookNode {
        node: Node::new(Default::default()),
        started: Arc::new(AtomicBool::new(false)),
        completed: Arc::new(AtomicBool::new(false)),
    };
    initiator.enable_on_connect().await;

    let target = TestNode::default();
    let target_addr = start_listening(&target).await;

    {
        // poll `connect` by hand and drop it the moment the connection registers, i.e. right in
        // the window where the OnConnect logic is being scheduled
        let mut connect_fut = std::pin::pin!(initiator.node().connect(target_addr));
        poll_fn(|cx| {
            if initiator.node().is_connected(target_addr) {
                return Poll::Ready(());
            }
            match connect_fut.as_mut().poll(cx) {
                Poll::Ready(res) => {
                    res.unwrap();
                    Poll::Ready(())
                }
                Poll::Pending => Poll::Pending,
            }
        })
        .await;
        // connect_fut is dropped here, cancelling it mid-scheduling
    }

    // the connection is established, so the hook must still fire for it
    assert!(initiator.node().is_connected(target_addr));
    wait_until(Duration::from_secs(1), || {
        initiator.started.load(Ordering::Acquire)
    })
    .await;

    // and, since it's ABORTABLE, a disconnect must still cut it short: its task must not have
    // been detached from the connection by the cancelled `connect`. The observation window
    // extends past the hook's full duration, so a detached (unaborted) hook is caught red-handed
    assert!(initiator.node().disconnect(target_addr).await);
    sleep(Duration::from_secs(1)).await;
    assert!(
        !initiator.completed.load(Ordering::Acquire),
        "the OnConnect task was detached by a cancelled connect"
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
        async fn on_disconnect(&self, _: SocketAddr, _: DisconnectOrigin) {
            sleep(Duration::from_secs(10)).await;
        }
    }

    let slow = SlowDisconnectNode(Node::new(Default::default()));
    slow.enable_on_disconnect().await;
    let slow_addr = slow.0.toggle_listener().await.unwrap().unwrap();

    let peer = TestNode::default();
    peer.node().connect(slow_addr).await.unwrap();

    wait_for_connections(slow.node(), 1).await;

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
async fn cancelled_disconnect_still_tears_down() {
    #[derive(Clone)]
    struct SlowDisconnectNode(Node);
    impl Pea2Pea for SlowDisconnectNode {
        fn node(&self) -> &Node {
            &self.0
        }
    }
    impl OnDisconnect for SlowDisconnectNode {
        // short enough that the final `shut_down` (which tears down the second connection and
        // thus waits out this timeout) fits its 2s budget below
        const TIMEOUT_MS: u64 = 1_000;
        async fn on_disconnect(&self, _: SocketAddr, _: DisconnectOrigin) {
            sleep(Duration::from_secs(10)).await;
        }
    }

    let slow = SlowDisconnectNode(Node::new(Default::default()));
    slow.enable_on_disconnect().await;
    let slow_addr = slow.0.toggle_listener().await.unwrap().unwrap();

    let peer = TestNode::default();
    peer.node().connect(slow_addr).await.unwrap();

    wait_for_connections(slow.node(), 1).await;
    let peer_addr = slow.0.connected_addrs()[0];

    // cancel the disconnect future while it's waiting on the slow hook; a disconnect, once
    // claimed, must complete its teardown regardless - otherwise the connection would become
    // permanently stuck (no other path can reclaim it) and `shut_down` would hang on it
    assert!(
        timeout(Duration::from_millis(100), slow.0.disconnect(peer_addr))
            .await
            .is_err()
    );

    // dropping the cancelled future performs the teardown synchronously
    assert!(!slow.0.is_connected(peer_addr));
    assert_eq!(slow.0.num_connected(), 0);

    // the teardown must have released the accounting (incl. the per-IP charge), so a new
    // connection from the same IP must be admitted right away
    let peer2 = TestNode::default();
    peer2.node().connect(slow_addr).await.unwrap();
    wait_for_connections(slow.node(), 1).await;

    // and the drain loop in `shut_down` must not wait on a stuck entry
    timeout(Duration::from_secs(2), slow.0.shut_down())
        .await
        .expect("shut_down hung after a cancelled disconnect");
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
        async fn on_disconnect(&self, _: SocketAddr, _: DisconnectOrigin) {
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

    wait_for_connections(b.node(), 1).await;

    a.node().disconnect(b_addr).await;

    wait_until(Duration::from_secs(1), || {
        a_fired.load(Ordering::Acquire) && b_fired.load(Ordering::Acquire)
    })
    .await;
}

#[tokio::test]
async fn disconnect_origin_reflects_source() {
    #[derive(Clone)]
    struct OriginNode {
        node: Node,
        // the DisconnectOrigin observed by *this* node's hook, if it fired
        seen: Arc<Mutex<Option<DisconnectOrigin>>>,
    }

    impl OriginNode {
        fn new(name: &str) -> Self {
            Self {
                node: Node::new(Config {
                    name: Some(name.into()),
                    ..Default::default()
                }),
                seen: Default::default(),
            }
        }
    }

    impl Pea2Pea for OriginNode {
        fn node(&self) -> &Node {
            &self.node
        }
    }

    impl OnDisconnect for OriginNode {
        async fn on_disconnect(&self, _: SocketAddr, origin: DisconnectOrigin) {
            *self.seen.lock() = Some(origin);
        }
    }

    // The passive side needs Reading so it notices the peer's FIN and tears down
    // via the read path. It's deliberately the *only* I/O protocol on that side:
    // with no Writing task, nothing races the read loop for ownership of the
    // disconnect, so the observed origin is unambiguous.
    impl Reading for OriginNode {
        type Message = BytesMut;
        type Codec = TestCodec<BytesMut>;
        fn codec(&self, _: SocketAddr, _: ConnectionSide) -> Self::Codec {
            Default::default()
        }
        async fn process_message(&self, _: SocketAddr, _: Self::Message) {}
    }

    let initiator = OriginNode::new("disconnector"); // the "us" side
    let responder = OriginNode::new("disconnectee"); // the "them" side

    // initiator enables neither Reading nor Writing: it's the one calling
    // disconnect(), so its own teardown can only be attributed to User.
    responder.enable_reading().await;
    initiator.enable_on_disconnect().await;
    responder.enable_on_disconnect().await;

    let responder_addr = start_listening(&responder).await;
    initiator.node().connect(responder_addr).await.unwrap();
    wait_for_connections(responder.node(), 1).await;

    // tear it down from the initiator
    assert!(initiator.node().disconnect(responder_addr).await);

    wait_until(Duration::from_secs(1), || {
        initiator.seen.lock().is_some() && responder.seen.lock().is_some()
    })
    .await;

    // "us": the side that called disconnect() sees User
    assert_eq!(*initiator.seen.lock(), Some(DisconnectOrigin::User));
    // "them": the side that only saw the FIN through its read loop sees Reading
    assert_eq!(*responder.seen.lock(), Some(DisconnectOrigin::Reading));
}
