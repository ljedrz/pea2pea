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
use parking_lot::Mutex;
use pea2pea::{
    Config, Connection, ConnectionSide, Node, Pea2Pea, Topology, connect_nodes, protocols::*,
};
use rand::RngExt;
use tokio::{
    net::{TcpListener, TcpSocket},
    sync::{Barrier, Notify, oneshot},
    task::JoinSet,
    time::{self, sleep},
};

use crate::common::{connect_and_wait, start_listening};

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

    wait_until(Duration::from_secs(1), || connectee.num_connected() == 1).await;

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

        wait_until(Duration::from_secs(1), || connector.num_connected() == 1).await;

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
        // Some(tx) on the connector side, None on the listener side
        gate: Arc<Mutex<Option<oneshot::Sender<()>>>>,
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
            if let Some(tx) = self.gate.lock().take() {
                let _ = tx.send(());
            }
            // ...and wait for the test to release us
            self.release.notified().await;
            Ok(conn)
        }
    }

    let (tx, started) = oneshot::channel();
    let release = Arc::new(Notify::new());

    let connector = GatedNode {
        node: Node::new(Default::default()),
        gate: Arc::new(Mutex::new(Some(tx))),
        release: release.clone(),
    };
    connector.enable_handshake().await;

    let target = crate::test_node!("target");
    let target_addr = start_listening(&target).await;

    let c = connector.clone();
    let join = tokio::spawn(async move { c.node().connect(target_addr).await });

    // wait for handshake to actually be running - no polling, no race
    started.await.unwrap();
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
async fn connector_conn_limit_breach_fails() {
    let config = Config {
        max_connections: 0,
        ..Default::default()
    };
    let connector = Node::new(config);
    let connectee = Node::new(Default::default());
    let connectee_addr = connectee.toggle_listener().await.unwrap().unwrap();

    assert!(connector.connect(connectee_addr).await.is_err());
}

#[tokio::test]
async fn connectee_conn_limit_breach_fails() {
    let config = Config {
        max_connections: 0,
        ..Default::default()
    };
    let connectee = Node::new(config);
    let connectee_addr = connectee.toggle_listener().await.unwrap().unwrap();

    let connector = Node::new(Default::default());

    // a breached connection limit doesn't close the listener, so this works
    connector.connect(connectee_addr).await.unwrap();

    // the number of connections on connectee side needs to be checked instead
    wait_until(Duration::from_secs(1), || connectee.num_connected() == 0).await;
}

#[tokio::test]
async fn connector_per_ip_conn_limit_breach_fails() {
    let config = Config {
        max_connections_per_ip: 1,
        ..Default::default()
    };
    let connector = Node::new(config);
    let connectee = Node::new(Default::default());
    let connectee_addr = connectee.toggle_listener().await.unwrap().unwrap();

    assert!(connector.connect(connectee_addr).await.is_ok());
    assert!(connector.connect(connectee_addr).await.is_err());
}

#[tokio::test]
async fn node_connectee_per_ip_conn_limit_breach_fails() {
    let config = Config {
        max_connections_per_ip: 1,
        ..Default::default()
    };
    let connectee = Node::new(config);
    let connectee_addr = connectee.toggle_listener().await.unwrap().unwrap();

    let connector1 = Node::new(Default::default());
    let connector2 = Node::new(Default::default());

    // a breached connection limit doesn't close the listener, so this works
    connector1.connect(connectee_addr).await.unwrap();
    connector2.connect(connectee_addr).await.unwrap();

    // the number of connections on connectee side needs to be checked instead
    wait_until(Duration::from_secs(1), || connectee.num_connected() == 1).await;
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
    wait_until(Duration::from_millis(500), || server.num_connected() == 1).await;

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
        let server_clone = server.clone();
        wait_until(Duration::from_millis(500), || {
            server_clone.num_connected() == 2
        })
        .await;

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
        wait_until(Duration::from_millis(500), || server.num_connected() == 1).await;
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

#[tokio::test(start_paused = true)]
async fn shutdown_closes_the_listener() {
    let node = Node::new(Default::default());
    let addr = node.toggle_listener().await.unwrap().unwrap();

    assert!(TcpListener::bind(addr).await.is_err());
    node.shut_down().await;
    time::advance(Duration::from_millis(100)).await;
    assert!(TcpListener::bind(addr).await.is_ok());
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
        // node: this is not the same as msgs_received in the Stats,
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
