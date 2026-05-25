mod common;

use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::{Bytes, BytesMut};
use common::wait_until;
use pea2pea::{
    Config, Connection, ConnectionSide, Node, Pea2Pea, Topology, connect_nodes, protocols::*,
};
use rand::RngExt;
use tokio::{
    net::{TcpListener, TcpSocket},
    sync::{Barrier, Notify},
    task::JoinSet,
    time::sleep,
};

use crate::common::{connect_and_wait, start_listening, wait_for_connections};

#[tokio::test]
async fn node_name_gets_auto_assigned() {
    let node = Node::new(Default::default());
    // ensure that a node without a given name get assigned a numeric ID
    let _: usize = node.config().name.as_ref().unwrap().parse().unwrap();
}

#[tokio::test]
async fn given_node_name_remains_unchanged() {
    let config = Config {
        name: Some("test".into()),
        ..Default::default()
    };
    let node = Node::new(config);
    // ensure that a node with a given name doesn't have it overwritten
    assert_eq!(node.config().name.as_ref().unwrap(), "test");
}

#[tokio::test]
async fn connect_using_custom_socket() {
    let connector = Node::new(Default::default());
    let connectee = Node::new(Default::default());
    let connectee_addr = connectee.toggle_listener().await.unwrap().unwrap();

    let socket = TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.77:0".parse().unwrap()).unwrap();

    connector
        .connect_using_socket(connectee_addr, socket)
        .await
        .unwrap();

    wait_for_connections(&connectee, 1).await;

    assert_eq!(
        connectee.connected_addrs()[0].ip(),
        "127.0.0.77".parse::<Ipv4Addr>().unwrap()
    );
}

#[tokio::test]
async fn listener_toggling() {
    let connector = Node::new(Default::default());
    let connectee = Node::new(Default::default());

    for _ in 0..3 {
        assert!(connectee.listening_addr().await.is_err());

        let connectee_addr = connectee.toggle_listener().await.unwrap().unwrap();

        connector.connect(connectee_addr).await.unwrap();

        wait_for_connections(&connector, 1).await;

        assert!(connectee.toggle_listener().await.unwrap().is_none());
        assert!(connectee.listening_addr().await.is_err());

        assert!(connector.disconnect(connectee_addr).await);
        assert!(connector.connect(connectee_addr).await.is_err());
    }
}

#[tokio::test]
async fn simple_connect_and_disconnect() {
    let nodes = Arc::new(common::start_test_nodes(2).await);
    connect_nodes(&nodes, Topology::Line).await.unwrap();
    let node1_addr = nodes[1].node().listening_addr().await.unwrap();

    wait_until(Duration::from_secs(1), || {
        nodes.iter().all(|n| n.node().num_connected() == 1)
    })
    .await;

    assert!(nodes.iter().all(|n| n.node().num_connecting() == 0));
    assert!(nodes[0].node().is_connected(node1_addr));
    assert!(nodes[0].node().disconnect(node1_addr).await);
    assert!(!nodes[0].node().is_connected(node1_addr));

    // node[1] didn't enable reading, so it has no way of knowing
    // that the connection has been broken by node[0]
    assert_eq!(nodes[0].node().num_connected(), 0);
    assert_eq!(nodes[1].node().num_connected(), 1);
}

#[tokio::test]
async fn is_connecting_is_observable() {
    #[derive(Clone)]
    struct GatedNode {
        node: Node,
        started: Arc<Notify>,
        release: Arc<Notify>,
    }
    impl Pea2Pea for GatedNode {
        fn node(&self) -> &Node {
            &self.node
        }
    }
    impl Handshake for GatedNode {
        async fn perform_handshake(&self, conn: Connection) -> io::Result<Connection> {
            // signal that we've reached the handshake...await.
            self.started.notify_one();
            // ...and wait for the test to release us
            self.release.notified().await;
            Ok(conn)
        }
    }

    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());

    let connector = GatedNode {
        node: Node::new(Default::default()),
        started: started.clone(),
        release: release.clone(),
    };
    connector.enable_handshake().await;

    let target = crate::test_node!("target");
    let target_addr = start_listening(&target).await;

    let c = connector.clone();
    let join = tokio::spawn(async move { c.node().connect(target_addr).await });

    // wait for handshake to actually be running - no polling, no race
    started.notified().await;
    assert!(connector.node().is_connecting(target_addr));
    assert!(!connector.node().is_connected(target_addr));

    release.notify_one();
    join.await.unwrap().unwrap();
    assert!(connector.node().is_connected(target_addr));
}

#[tokio::test]
async fn self_connection_fails() {
    let node = Node::new(Default::default());
    let own_addr = node.toggle_listener().await.unwrap().unwrap();
    assert!(node.connect(own_addr).await.is_err());
}

#[tokio::test]
async fn duplicate_connection_fails() {
    let nodes = common::start_test_nodes(2).await;
    assert!(connect_nodes(&nodes, Topology::Line).await.is_ok());
    assert!(connect_nodes(&nodes, Topology::Line).await.is_err());
}

#[tokio::test]
async fn allowed_duplicate_connection_works() {
    let config = Config {
        allow_duplicate_connections: true,
        ..Default::default()
    };
    let connector = Node::new(config);
    let connectee = Node::new(Default::default());
    let connectee_addr = connectee.toggle_listener().await.unwrap().unwrap();

    assert!(connector.connect(connectee_addr).await.is_ok());
    assert!(connector.connect(connectee_addr).await.is_ok());
}

#[tokio::test]
async fn two_way_connection_works() {
    let mut nodes = common::start_test_nodes(2).await;
    assert!(connect_nodes(&nodes, Topology::Line).await.is_ok());
    nodes.reverse();
    assert!(connect_nodes(&nodes, Topology::Line).await.is_ok());
}

#[tokio::test]
async fn conn_limit_rejects_on_both_sides() {
    // outbound: connector with max_connections=0 refuses its own connect()
    let connector = Node::new(Config {
        max_connections: 0,
        ..Default::default()
    });
    let target = Node::new(Default::default());
    let target_addr = target.toggle_listener().await.unwrap().unwrap();
    assert!(connector.connect(target_addr).await.is_err());

    // inbound: connectee with max_connections=0 accepts then drops
    let connectee = Node::new(Config {
        max_connections: 0,
        ..Default::default()
    });
    let connectee_addr = connectee.toggle_listener().await.unwrap().unwrap();
    let connector = Node::new(Default::default());
    connector.connect(connectee_addr).await.unwrap();
    sleep(Duration::from_millis(50)).await;
    assert_eq!(connectee.num_connected(), 0);
}

#[tokio::test]
async fn max_connections_per_ip_is_distinct() {
    let config = Config {
        name: Some("server".into()),
        max_connections_per_ip: 1,
        max_connections: 10, // ensure only per-IP limit triggers
        ..Default::default()
    };
    let server = Node::new(config);
    let server_addr = server.toggle_listener().await.unwrap().unwrap();

    // connector A (default IP: 127.0.0.1)
    let connector_a1 = Node::new(Default::default());
    connector_a1.connect(server_addr).await.unwrap();

    // wait for A1 to connect
    wait_for_connections(&server, 1).await;

    // connector A2 (default IP: 127.0.0.1) -> should fail (limit reached for this IP)
    let connector_a2 = Node::new(Default::default());
    connector_a2.connect(server_addr).await.unwrap();

    // connector B (spoofed IP: 127.0.0.2) -> should succeed (different IP)
    // note: the loopback interface (lo) typically accepts the entire 127.0.0.0/8 block;
    //       we bind explicitly to 127.0.0.2 to simulate a different machine.
    let socket = TcpSocket::new_v4().unwrap();
    // verify OS allows binding to non-127.0.0.1 loopback (usually works)
    if socket.bind("127.0.0.2:0".parse().unwrap()).is_ok() {
        let connector_b = Node::new(Default::default());
        connector_b
            .connect_using_socket(server_addr, socket)
            .await
            .unwrap();

        // total connections should be 2:
        // - one from 127.0.0.1 (A1)
        // - one from 127.0.0.2 (B)
        // - A2 should have been dropped
        wait_for_connections(&server, 2).await;

        // verify specifically who is connected
        let connected_ips: Vec<_> = server
            .connected_addrs()
            .iter()
            .map(|a| a.ip().to_string())
            .collect();
        assert!(connected_ips.contains(&"127.0.0.1".to_string()));
        assert!(connected_ips.contains(&"127.0.0.2".to_string()));
    } else {
        println!("Skipping 127.0.0.2 test portion due to OS binding restrictions");
        // fallback: just assert A2 failed (num_connected stays 1)
        wait_for_connections(&server, 1).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn overlapping_duplicate_connection_attempts_fail() {
    const NUM_ATTEMPTS: usize = 5;

    let connector = Node::new(Default::default());

    let connectee = Node::new(Default::default());
    let connectee_addr = connectee.toggle_listener().await.unwrap().unwrap();

    let err_count = Arc::new(AtomicUsize::new(0));
    for _ in 0..NUM_ATTEMPTS {
        let connector_clone = connector.clone();
        let err_count_clone = err_count.clone();
        tokio::spawn(async move {
            if connector_clone.connect(connectee_addr).await.is_err() {
                err_count_clone.fetch_add(1, Ordering::Relaxed);
            }
        });
    }

    wait_until(Duration::from_secs(1), || {
        err_count.load(Ordering::Relaxed) == NUM_ATTEMPTS - 1
    })
    .await;
}

#[tokio::test]
async fn shutdown_closes_the_listener() {
    let node = Node::new(Default::default());
    let addr = node.toggle_listener().await.unwrap().unwrap();

    assert!(TcpListener::bind(addr).await.is_err());
    node.shut_down().await;
    wait_until(Duration::from_secs(1), || {
        std::net::TcpListener::bind(addr).is_ok()
    })
    .await;
}

#[tokio::test]
async fn test_nodes_use_localhost() {
    let node = Node::new(Default::default());
    let addr = node.toggle_listener().await.unwrap().unwrap();
    assert_eq!(addr.ip(), Ipv4Addr::LOCALHOST);
}

#[tokio::test]
async fn connection_timeout_works() {
    // configure a very short connection timeout (200ms)
    let config = Config {
        name: Some("impatient".into()),
        connection_timeout_ms: 200,
        ..Default::default()
    };
    let node = Node::new(config);

    // attempt to connect to a non-routable IP (TEST-NET-1)
    // 192.0.2.x is reserved for documentation and examples; it should typically blackhole
    // or at least not respond with a TCP RST immediately, triggering the timeout logic
    let blackhole_addr = "192.0.2.1:1234".parse().unwrap();

    let start = Instant::now();

    // perform the connection attempt
    let result = node.connect(blackhole_addr).await;

    let elapsed = start.elapsed();

    // verify the result
    assert!(result.is_err());
    let err = result.unwrap_err();

    // check that it is indeed a TimedOut error, not a "Network Unreachable" or "Refused"
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);

    // verify the timing
    // OS TCP timeouts are usually 30s+, so if this finishes in <1s, we know our logic worked
    assert!(elapsed >= Duration::from_millis(200));
    assert!(elapsed < Duration::from_secs(2));
}

#[tokio::test(flavor = "multi_thread")]
async fn max_connections_race_condition() {
    // a minimal target node
    #[derive(Clone)]
    struct TestNode(Node);

    impl Pea2Pea for TestNode {
        fn node(&self) -> &Node {
            &self.0
        }
    }

    // configure the target Node with a strict limit
    let limit = 3;
    let target_config = Config {
        max_connections: limit,
        ..Default::default()
    };

    let target_node = TestNode(Node::new(target_config));
    let target_addr = start_listening(&target_node).await;

    // create many more nodes than the limit allows
    let swarm_size = 50;
    let mut swarm_nodes = Vec::new();

    for _ in 0..swarm_size {
        let node = Node::new(Default::default());
        swarm_nodes.push(node);
    }

    // spawn tasks to ensure the connection attempts happen in parallel
    let mut conn_tasks = JoinSet::new();
    let barrier = Arc::new(Barrier::new(swarm_size));
    for node in swarm_nodes {
        let b = barrier.clone();
        conn_tasks.spawn(async move {
            // synchronize all attempts
            b.wait().await;
            // ignore the result; we expect some to fail
            let _ = node.connect(target_addr).await;
        });
    }

    // wait for all attempts to resolve
    while conn_tasks.join_next().await.is_some() {}

    // give the target node's event loop a moment to process everything
    sleep(Duration::from_millis(500)).await;

    // check for races
    let current_peers = target_node.node().num_connected();
    assert!(
        current_peers <= limit as usize,
        "Race Condition Detected: Target accepted {} connections, but limit was {}",
        current_peers,
        limit
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn max_connecting_limit_rejects_new_attempts() {
    const MAX_CONNECTING: u16 = 3;

    // a node whose handshake intentionally stalls so that the `connecting`
    // slot stays held for the whole test
    #[derive(Clone)]
    struct StallingHandshakeNode(Node);
    impl Pea2Pea for StallingHandshakeNode {
        fn node(&self) -> &Node {
            &self.0
        }
    }
    impl Handshake for StallingHandshakeNode {
        const TIMEOUT_MS: u64 = 60_000;
        async fn perform_handshake(
            &self,
            conn: pea2pea::Connection,
        ) -> io::Result<pea2pea::Connection> {
            sleep(Duration::from_secs(30)).await;
            Ok(conn)
        }
    }

    let config = Config {
        name: Some("connector".into()),
        max_connecting: MAX_CONNECTING,
        max_connections: 100,        // permissive
        max_connections_per_ip: 100, // permissive
        ..Default::default()
    };
    let connector = StallingHandshakeNode(Node::new(config));
    connector.enable_handshake().await;

    // bring up enough targets that each connect goes to a distinct addr
    let mut targets = Vec::with_capacity(MAX_CONNECTING as usize + 1);
    for _ in 0..=MAX_CONNECTING {
        let t = Node::new(Default::default());
        let addr = t.toggle_listener().await.unwrap().unwrap();
        targets.push((t, addr));
    }

    // spawn MAX_CONNECTING connects; each will stick in the handshake
    for (_, addr) in targets.iter().take(MAX_CONNECTING as usize) {
        let connector = connector.clone();
        let addr = *addr;
        tokio::spawn(async move {
            let _ = connector.0.connect(addr).await;
        });
    }

    // wait until the connecting set is full
    wait_until(Duration::from_secs(2), || {
        connector.0.num_connecting() == MAX_CONNECTING as usize
    })
    .await;

    // the next attempt must fail synchronously with QuotaExceeded
    let extra_addr = targets.last().unwrap().1;
    let err = connector.0.connect(extra_addr).await.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::QuotaExceeded);

    // and the connecting count did not change as a result
    assert_eq!(connector.0.num_connecting(), MAX_CONNECTING as usize);

    // tidy up - the stalled connect tasks would otherwise keep the runtime busy
    connector.0.shut_down().await;
}

#[tokio::test]
async fn toggle_listener_without_addr_errors() {
    let config = Config {
        listener_addr: None,
        ..Default::default()
    };
    let node = Node::new(config);

    let err = node.toggle_listener().await.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::AddrNotAvailable);
}

#[tokio::test]
async fn disconnect_unknown_addr_returns_false() {
    let node = Node::new(Default::default());
    let bogus: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
    assert!(!node.disconnect(bogus).await);
}

#[tokio::test]
async fn shut_down_is_idempotent() {
    let node = Node::new(Default::default());
    node.toggle_listener().await.unwrap();

    // back-to-back shutdowns must not panic, hang, or leave the node in a bad state
    node.shut_down().await;
    node.shut_down().await;
}

#[tokio::test]
async fn connect_after_shut_down_is_rejected() {
    let connector = Node::new(Default::default());

    let target = Node::new(Default::default());
    let target_addr = target.toggle_listener().await.unwrap().unwrap();

    connector.shut_down().await;

    // shut_down() should make subsequent connects fail immediately
    let err = connector.connect(target_addr).await.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::Other);
    // and no slot is taken
    assert_eq!(connector.num_connecting(), 0);
    assert_eq!(connector.num_connected(), 0);
}

#[tokio::test]
async fn connection_info_side_reflects_role() {
    let nodes = common::start_test_nodes(2).await;
    let listener_addr = nodes[1].node().listening_addr().await.unwrap();

    nodes[0].node().connect(listener_addr).await.unwrap();

    wait_for_connections(nodes[0].node(), 1).await;
    wait_for_connections(nodes[1].node(), 1).await;

    // From the initiator's perspective, its peer is the Responder...
    let info = nodes[0].node().connection_info(listener_addr).unwrap();
    assert_eq!(info.side(), ConnectionSide::Responder);

    // ...and vice versa.
    let initiator_addr = nodes[1].node().connected_addrs()[0];
    let info = nodes[1].node().connection_info(initiator_addr).unwrap();
    assert_eq!(info.side(), ConnectionSide::Initiator);
}

#[tokio::test]
async fn connection_info_unknown_addr_returns_none() {
    let node = Node::new(Default::default());
    let bogus: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
    assert!(node.connection_info(bogus).is_none());
    assert!(node.connection_infos().is_empty());
}

#[tokio::test]
async fn message_stats() {
    let mut rng = rand::rng();

    let reader = crate::test_node!("reader");
    let reader_addr = start_listening(&reader).await;
    reader.enable_reading().await;

    let writer = crate::test_node!("writer");
    writer.enable_writing().await;

    writer.node().connect(reader_addr).await.unwrap();

    let sent_msgs_count = rng.random_range(2..=64); // shouldn't exceed the outbound queue depth
    let mut msg = vec![0u8; rng.random_range(1..=4096)];
    rng.fill(&mut msg[..]);

    let msg = Bytes::from(msg);

    for _ in 0..sent_msgs_count {
        writer
            .unicast(reader_addr, msg.clone())
            .unwrap()
            .await
            .unwrap()
            .unwrap();
    }

    // 2 is the common test length prefix size
    let expected_msgs_size = sent_msgs_count * (2 + msg.len() as u64);

    assert_eq!(
        writer.node().stats().sent(),
        (sent_msgs_count, expected_msgs_size)
    );

    wait_until(Duration::from_secs(1), || {
        let conn_info = writer.node().connection_info(reader_addr).unwrap();
        let (sent_msgs, sent_bytes) = conn_info.stats().sent();
        sent_msgs == sent_msgs_count && sent_bytes == expected_msgs_size
    })
    .await;

    wait_until(Duration::from_secs(1), || {
        reader.node().stats().received() == (sent_msgs_count, expected_msgs_size)
    })
    .await;

    let writer_addr = reader.node().connected_addrs()[0];
    wait_until(Duration::from_secs(1), || {
        let conn_info = reader.node().connection_info(writer_addr).unwrap();
        let (received_msgs, received_bytes) = conn_info.stats().received();
        received_msgs == sent_msgs_count && received_bytes == expected_msgs_size
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn drops_messages_wo_backpressure() {
    #[derive(Clone)]
    struct RealtimeNode {
        node: Node,
        // note: this is not the same as msgs_received in the Stats,
        // which is incremented even if the message is dropped due to
        // inbound queue saturation
        num_processed_messages: Arc<AtomicU8>,
    }

    impl Pea2Pea for RealtimeNode {
        fn node(&self) -> &Node {
            &self.node
        }
    }

    impl Reading for RealtimeNode {
        const MESSAGE_QUEUE_DEPTH: usize = 1;
        const BACKPRESSURE: bool = false;

        type Message = BytesMut;
        type Codec = common::TestCodec<BytesMut>;

        async fn process_message(&self, _: SocketAddr, _: Self::Message) {
            self.num_processed_messages.fetch_add(1, Ordering::Relaxed);
        }

        fn codec(&self, _: SocketAddr, _: ConnectionSide) -> <Self as Reading>::Codec {
            Default::default()
        }
    }

    const MSGS_TO_SEND: u8 = 25;

    let rt_node = RealtimeNode {
        node: Node::new(Config {
            name: Some("realtime".into()),
            ..Default::default()
        }),
        num_processed_messages: Default::default(),
    };
    let fast_node = crate::test_node!("fastboi");

    rt_node.enable_reading().await;
    fast_node.enable_writing().await;

    let rt_node_addr = start_listening(&rt_node).await;
    fast_node.node().connect(rt_node_addr).await.unwrap();

    for _ in 0..MSGS_TO_SEND {
        let _ = fast_node.unicast_fast(rt_node_addr, (&b"gotta go fast!"[..]).into());
    }

    // ensure that the realtime node has time to attempt to process everything
    wait_until(Duration::from_secs(5), || {
        rt_node.node().stats().received().0 == MSGS_TO_SEND as u64
    })
    .await;

    // the number of processed messages should be lower than that of sent messages
    let processed = rt_node.num_processed_messages.load(Ordering::Relaxed);
    assert!(processed < MSGS_TO_SEND);
}

#[tokio::test]
async fn outbound_queue_saturation() {
    #[derive(Clone)]
    struct SaturatedNode(Node);

    impl Pea2Pea for SaturatedNode {
        fn node(&self) -> &Node {
            &self.0
        }
    }

    impl Writing for SaturatedNode {
        const MESSAGE_QUEUE_DEPTH: usize = 1;

        type Message = Bytes;
        type Codec = common::TestCodec<Bytes>;

        fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
            Default::default()
        }
    }

    let sender = SaturatedNode(Node::new(Config {
        name: Some("sender".into()),
        ..Default::default()
    }));
    let receiver = crate::test_node!("lazy_receiver");

    sender.enable_writing().await;
    // note: receiver does not enable reading, forcing TCP backpressure

    let receiver_addr = start_listening(&receiver).await;
    sender.node().connect(receiver_addr).await.unwrap();

    // give the connection a moment to finalize
    sleep(Duration::from_millis(50)).await;

    let msg = Bytes::from(&b"spam"[..]);
    let mut saturated = false;

    // spam messages until the queue fills up
    for _ in 0..10 {
        match sender.unicast_fast(receiver_addr, msg.clone()) {
            Ok(_) => {
                // message queued successfully, keep spamming
            }
            Err(e) => {
                // validate that the error is indeed due to queue saturation
                if e.kind() == io::ErrorKind::QuotaExceeded {
                    saturated = true;
                    break;
                } else {
                    panic!("unexpected error type during saturation test: {e}");
                }
            }
        }
    }

    assert!(saturated);
}

#[tokio::test]
async fn idle_timeout_drops_silent_connection() {
    #[derive(Clone)]
    struct ImpatientReader(pea2pea::Node);
    impl Pea2Pea for ImpatientReader {
        fn node(&self) -> &pea2pea::Node {
            &self.0
        }
    }
    impl Reading for ImpatientReader {
        const IDLE_TIMEOUT_MS: u64 = 250;

        type Message = BytesMut;
        type Codec = common::TestCodec<BytesMut>;

        fn codec(&self, _: std::net::SocketAddr, _: ConnectionSide) -> Self::Codec {
            Default::default()
        }

        async fn process_message(&self, _: std::net::SocketAddr, _: Self::Message) {}
    }

    let reader = ImpatientReader(pea2pea::Node::new(Default::default()));
    reader.enable_reading().await;
    let reader_addr = reader.0.toggle_listener().await.unwrap().unwrap();

    let peer = crate::test_node!("quiet_peer");
    peer.node().connect(reader_addr).await.unwrap();

    wait_for_connections(reader.node(), 1).await;

    // no traffic - the reader should disconnect roughly IDLE_TIMEOUT_MS later
    let start = Instant::now();
    wait_for_connections(reader.node(), 0).await;
    let elapsed = start.elapsed();

    assert!(
        elapsed >= Duration::from_millis(200),
        "disconnected too quickly: {elapsed:?}"
    );
}

/// `IDLE_TIMEOUT_MS = 0` opts out of the timeout entirely; an otherwise-silent
/// connection should remain up indefinitely.
#[tokio::test]
async fn idle_timeout_zero_disables_timeout() {
    #[derive(Clone)]
    struct PatientReader(pea2pea::Node);
    impl Pea2Pea for PatientReader {
        fn node(&self) -> &pea2pea::Node {
            &self.0
        }
    }
    impl Reading for PatientReader {
        const IDLE_TIMEOUT_MS: u64 = 0;

        type Message = BytesMut;
        type Codec = common::TestCodec<BytesMut>;

        fn codec(&self, _: std::net::SocketAddr, _: ConnectionSide) -> Self::Codec {
            Default::default()
        }

        async fn process_message(&self, _: std::net::SocketAddr, _: Self::Message) {}
    }

    let reader = PatientReader(pea2pea::Node::new(Default::default()));
    reader.enable_reading().await;
    let reader_addr = reader.0.toggle_listener().await.unwrap().unwrap();

    let peer = crate::test_node!("silent_peer");
    peer.node().connect(reader_addr).await.unwrap();

    wait_for_connections(reader.node(), 1).await;

    // wait longer than any reasonable per-test timeout
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(reader.0.num_connected(), 1);
}

/// `Writing::TIMEOUT_MS` bounds the per-message `flush()` call. With a
/// peer that never reads, the sender's flush eventually stalls on TCP
/// backpressure and the timeout must propagate via the delivery `oneshot`,
/// tearing the connection down.
///
/// Note: a default loopback SNDBUF/RCVBUF combo on Linux/macOS lets several
/// MB of data pipeline before flush() actually blocks, which makes a "send
/// big messages and hope" approach flaky on machines with autotuned
/// buffers. We instead shrink the sender's send buffer via
/// `connect_using_socket` so the writer's flush stalls almost immediately
/// once the receiver stops accepting bytes. This is the same SO_SNDBUF
/// knob the `Handshake` docs already point at as the right place for
/// socket-option tuning.
#[tokio::test]
async fn write_timeout_propagates_to_delivery() {
    use tokio::net::TcpSocket;

    #[derive(Clone)]
    struct ImpatientWriter(pea2pea::Node);
    impl Pea2Pea for ImpatientWriter {
        fn node(&self) -> &pea2pea::Node {
            &self.0
        }
    }
    impl Writing for ImpatientWriter {
        const TIMEOUT_MS: u64 = 200;
        // queue must be deep enough that the *flush*, not try_send, is the
        // first thing to stall
        const MESSAGE_QUEUE_DEPTH: usize = 128;

        type Message = Bytes;
        type Codec = common::TestCodec<Bytes>;

        fn codec(&self, _: std::net::SocketAddr, _: ConnectionSide) -> Self::Codec {
            Default::default()
        }
    }

    let sender = ImpatientWriter(pea2pea::Node::new(Default::default()));
    sender.enable_writing().await;

    // a receiver that never reads -> kernel recv buffer eventually fills
    let receiver = crate::test_node!("blackhole");
    let receiver_addr = receiver.node().toggle_listener().await.unwrap().unwrap();

    // shrink the sender's send buffer so its writer flush stalls within a
    // handful of messages regardless of the receiver's recv-buffer tuning
    let socket = TcpSocket::new_v4().unwrap();
    // 4kB requested; Linux typically doubles this and clamps to wmem_min
    let _ = socket.set_send_buffer_size(4096);
    sender
        .0
        .connect_using_socket(receiver_addr, socket)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 60_000-byte messages (TestCodec's 2-byte length prefix puts the
    // hard ceiling at 65_535 per message). With the shrunken SNDBUF, only
    // a couple of flushes complete before flush() stalls and TIMEOUT_MS
    // fires.
    let big = Bytes::from(vec![0xAB; 60_000]);
    let mut delivery_rxs = Vec::new();
    for _ in 0..32 {
        let Ok(rx) = sender.unicast(receiver_addr, big.clone()) else {
            break;
        };
        delivery_rxs.push(rx);
    }

    let mut saw_timeout = false;
    for rx in delivery_rxs {
        // once the writer task hits a write error it breaks out of its
        // recv loop; the remaining queued WrappedMessages get dropped
        // along with their oneshot senders, so `rx.await` for those
        // returns RecvError. That's expected - stop looking.
        let Ok(res) = rx.await else { break };
        match res {
            Ok(()) => continue,
            Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                saw_timeout = true;
                break;
            }
            Err(e) => panic!("unexpected write error: {e}"),
        }
    }

    assert!(
        saw_timeout,
        "expected a write to time out under TCP backpressure"
    );

    // the writer task breaks out of its loop on error, which fires
    // DisconnectOnDrop -> the connection must be gone shortly after
    wait_for_connections(sender.node(), 0).await;
}

/// `unicast` / `unicast_fast` / `broadcast` must return `Unsupported`
/// when `Writing` is not enabled.
#[tokio::test]
async fn writing_methods_return_unsupported_when_not_enabled() {
    let node = crate::test_node!("mute");
    // intentionally not calling enable_writing()

    let any: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
    let dummy = Bytes::from(&b"x"[..]);

    let err = node.unicast_fast(any, dummy.clone()).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::Unsupported);

    let err = node.unicast(any, dummy.clone()).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::Unsupported);

    let err = node.broadcast(dummy).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::Unsupported);
}

/// `unicast` (with delivery confirmation) returns `NotConnected` for an
/// unknown peer - the `unicast_fast` variant of this is already covered
/// by `drop_connection_on_invalid_message`.
#[tokio::test]
async fn unicast_returns_not_connected_for_unknown_peer() {
    let sender = crate::test_node!("sender");
    sender.enable_writing().await;

    let unconnected: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
    let err = sender
        .unicast(unconnected, Bytes::from(&b"x"[..]))
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::NotConnected);
}

#[tokio::test]
async fn simple_broadcast() {
    let random_nodes = common::start_test_nodes(4).await;
    for rando in &random_nodes {
        rando.enable_reading().await;
    }

    let broadcaster = crate::test_node!("chatty");
    broadcaster.enable_writing().await;

    let chatty = broadcaster.clone();
    tokio::spawn(async move {
        let message = "hello there ( ͡° ͜ʖ ͡°)";
        let bytes = Bytes::from(message.as_bytes());

        loop {
            if chatty.node().num_connected() != 0 {
                chatty.broadcast(bytes.clone()).unwrap();
            }

            sleep(Duration::from_millis(50)).await;
        }
    });

    for rando in &random_nodes {
        broadcaster
            .node()
            .connect(rando.node().listening_addr().await.unwrap())
            .await
            .unwrap();
    }

    wait_until(Duration::from_secs(1), || {
        random_nodes
            .iter()
            .all(|rando| rando.node().stats().received().0 >= 2)
    })
    .await;
}

#[tokio::test]
async fn broadcast_with_no_peers_is_ok() {
    let node = crate::test_node!("loner");
    node.enable_writing().await;
    // no connections at all - should still return Ok
    assert!(
        node.broadcast(Bytes::from(&b"echoes in the void"[..]))
            .is_ok()
    );
}

#[tokio::test]
async fn broadcast_continues_past_saturated_peer() {
    // A broadcaster with depth=1 makes per-peer queues trivial to fill.
    #[derive(Clone)]
    struct TinyQueueSender(Node);
    impl Pea2Pea for TinyQueueSender {
        fn node(&self) -> &Node {
            &self.0
        }
    }
    impl Writing for TinyQueueSender {
        const MESSAGE_QUEUE_DEPTH: usize = 1;
        type Message = Bytes;
        type Codec = common::TestCodec<Bytes>;
        fn codec(&self, _: SocketAddr, _: ConnectionSide) -> Self::Codec {
            Default::default()
        }
    }

    let sender = TinyQueueSender(Node::new(Default::default()));
    sender.enable_writing().await;

    // a peer that won't read -> its queue + kernel buffer saturate
    let slow = crate::test_node!("slow");
    let slow_addr = slow.node().toggle_listener().await.unwrap().unwrap();

    // a peer that will read normally
    let fast = crate::test_node!("fast");
    fast.enable_reading().await;
    let fast_addr = fast.node().toggle_listener().await.unwrap().unwrap();

    sender.0.connect(slow_addr).await.unwrap();
    sender.0.connect(fast_addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // saturate the slow peer's pipeline with large messages until try_send fails
    let big = Bytes::from(vec![0xAB; 60_000]);
    let mut slow_saturated = false;
    for _ in 0..200 {
        if !sender.0.is_connected(slow_addr) {
            break; // shouldn't happen, but be defensive
        }
        if sender.unicast_fast(slow_addr, big.clone()).is_err() {
            slow_saturated = true;
            break;
        }
    }
    assert!(
        slow_saturated,
        "couldn't saturate the slow peer's outbound queue"
    );

    let baseline = fast.node().stats().received().0;

    // The broadcast: slow peer drops it (queue full), fast peer must still receive.
    sender
        .broadcast(Bytes::from(&b"to all who can hear me"[..]))
        .unwrap();

    wait_until(Duration::from_secs(2), || {
        fast.node().stats().received().0 > baseline
    })
    .await;
}

#[tokio::test]
async fn line_conn_counts() {
    const N: usize = 10;

    let nodes = common::start_test_nodes(N).await;
    connect_and_wait(&nodes, Topology::Line).await.unwrap();

    assert!(nodes.iter().enumerate().all(|(i, node)| {
        if i == 0 || i == N - 1 {
            node.node().num_connected() == 1
        } else {
            node.node().num_connected() == 2
        }
    }));
}

#[tokio::test]
async fn ring_conn_counts() {
    const N: usize = 10;

    let nodes = common::start_test_nodes(N).await;
    connect_and_wait(&nodes, Topology::Ring).await.unwrap();

    assert!(nodes.iter().all(|node| node.node().num_connected() == 2));
}

#[tokio::test]
async fn mesh_conn_counts() {
    const N: usize = 10;

    let nodes = common::start_test_nodes(N).await;
    connect_and_wait(&nodes, Topology::Mesh).await.unwrap();

    assert!(nodes.iter().all(|n| n.node().num_connected() == N - 1));
}

#[tokio::test]
async fn star_conn_counts() {
    const N: usize = 10;

    let nodes = common::start_test_nodes(N).await;
    connect_and_wait(&nodes, Topology::Star).await.unwrap();

    assert!(nodes.iter().enumerate().all(|(i, node)| {
        if i == 0 {
            node.node().num_connected() == N - 1
        } else {
            node.node().num_connected() == 1
        }
    }));
}

#[tokio::test]
async fn grid_conn_counts() {
    const N: usize = 10;

    let nodes = common::start_test_nodes(N).await;
    let topology = Topology::Grid {
        width: 5,
        height: 2,
    };
    connect_and_wait(&nodes, topology).await.unwrap();

    assert!(nodes.iter().enumerate().all(|(i, node)| {
        let row = i / 5;
        let col = i % 5;
        let mut expected = 0;

        // check Up
        if row > 0 {
            expected += 1;
        }
        // check Down (height is 2, so max row is 1)
        if row < 1 {
            expected += 1;
        }
        // check Left
        if col > 0 {
            expected += 1;
        }
        // check Right (width is 5, so max col is 4)
        if col < 4 {
            expected += 1;
        }

        node.node().num_connected() == expected
    }));
}

#[tokio::test]
async fn tree_conn_counts() {
    const N: usize = 10;

    let nodes = common::start_test_nodes(N).await;
    connect_and_wait(&nodes, Topology::Tree).await.unwrap();

    assert!(nodes.iter().enumerate().all(|(i, node)| {
        let mut expected = 0;

        // check for parent (Node 0 has no parent)
        if i > 0 {
            expected += 1;
        }

        // check for left child
        if 2 * i + 1 < N {
            expected += 1;
        }

        // check for right child
        if 2 * i + 2 < N {
            expected += 1;
        }

        node.node().num_connected() == expected
    }));
}

#[tokio::test]
async fn random_conn_counts() {
    const N: usize = 10;

    let nodes = common::start_test_nodes(N).await;
    let topology = Topology::Random {
        degree: 2,
        seed: 12345,
    };
    connect_and_wait(&nodes, topology).await.unwrap();

    // verify every node has at least the minimum degree (2)
    assert!(nodes.iter().all(|node| node.node().num_connected() >= 2));

    // verify the global total of connections matches the expectation
    let total_connections: usize = nodes.iter().map(|n| n.node().num_connected()).sum();

    // N (10) * degree (2) * 2 sides = 40
    assert_eq!(total_connections, N * 2 * 2);
}

#[tokio::test]
async fn connect_nodes_with_too_few_nodes_errors() {
    // zero nodes
    let nodes: Vec<common::TestNode> = vec![];
    let err = pea2pea::connect_nodes(&nodes, Topology::Line)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);

    // one node
    let nodes = common::start_test_nodes(1).await;
    let err = pea2pea::connect_nodes(&nodes, Topology::Mesh)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[tokio::test]
async fn topology_grid_dimension_mismatch_errors() {
    // 5 nodes, but a 2x3 grid wants 6
    let nodes = common::start_test_nodes(5).await;
    let err = pea2pea::connect_nodes(
        &nodes,
        Topology::Grid {
            width: 2,
            height: 3,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[tokio::test]
async fn topology_random_degree_too_high_errors() {
    let nodes = common::start_test_nodes(3).await;
    // degree must be < N (3); 3 is invalid
    let err = pea2pea::connect_nodes(
        &nodes,
        Topology::Random {
            degree: 3,
            seed: 42,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[tokio::test]
async fn num_expected_connections_zero_nodes() {
    // every topology should agree that 0 nodes => 0 connections,
    // without panicking on the `num_nodes - 1` arithmetic
    assert_eq!(Topology::Line.num_expected_connections(0), 0);
    assert_eq!(Topology::Ring.num_expected_connections(0), 0);
    assert_eq!(Topology::Mesh.num_expected_connections(0), 0);
    assert_eq!(Topology::Star.num_expected_connections(0), 0);
    assert_eq!(
        Topology::Grid {
            width: 0,
            height: 0
        }
        .num_expected_connections(0),
        0
    );
    assert_eq!(Topology::Tree.num_expected_connections(0), 0);
    assert_eq!(
        Topology::Random { degree: 0, seed: 0 }.num_expected_connections(0),
        0
    );
}
