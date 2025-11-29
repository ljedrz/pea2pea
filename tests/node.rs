use deadline::deadline;
use tokio::{
    net::{TcpListener, TcpSocket},
    sync::Barrier,
    task::JoinSet,
    time::sleep,
};

mod common;
use std::{
    io,
    net::Ipv4Addr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use pea2pea::{Config, Node, Pea2Pea, Topology, connect_nodes, protocols::Handshake};

impl Handshake for common::TestNode {
    async fn perform_handshake(
        &self,
        conn: pea2pea::Connection,
    ) -> io::Result<pea2pea::Connection> {
        sleep(Duration::from_millis(50)).await;
        Ok(conn)
    }
}

#[tokio::test]
async fn node_creation_any_port_works() {
    let _node = Node::new(Default::default());
}

#[tokio::test]
async fn node_name_gets_auto_assigned() {
    let node = Node::new(Default::default());
    // ensure that a node without a given name get assigned a numeric ID
    let _: usize = node.config().name.as_ref().unwrap().parse().unwrap();
}

#[tokio::test]
async fn node_given_name_remains_unchanged() {
    let config = Config {
        name: Some("test".into()),
        ..Default::default()
    };
    let node = Node::new(config);
    // ensure that a node with a given name doesn't have it overwritten
    assert_eq!(node.config().name.as_ref().unwrap(), "test");
}

#[tokio::test]
async fn node_use_provided_socket() {
    let connector = Node::new(Default::default());
    let connectee = Node::new(Default::default());
    let connectee_addr = connectee.toggle_listener().await.unwrap().unwrap();

    let socket = TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.77:0".parse().unwrap()).unwrap();

    connector
        .connect_using_socket(connectee_addr, socket)
        .await
        .unwrap();

    let connectee_clone = connectee.clone();
    deadline!(Duration::from_secs(1), move || connectee_clone
        .num_connected()
        == 1);

    assert_eq!(
        connectee.connected_addrs()[0].ip(),
        "127.0.0.77".parse::<Ipv4Addr>().unwrap()
    );
}

#[tokio::test]
async fn node_listener_toggling() {
    let connector = Node::new(Default::default());
    let connectee = Node::new(Default::default());

    for _ in 0..3 {
        assert!(connectee.listening_addr().await.is_err());

        let connectee_addr = connectee.toggle_listener().await.unwrap().unwrap();

        connector.connect(connectee_addr).await.unwrap();

        let connectee_clone = connectee.clone();
        deadline!(Duration::from_secs(1), move || connectee_clone
            .num_connected()
            == 1);

        assert!(connectee.toggle_listener().await.unwrap().is_none());
        assert!(connectee.listening_addr().await.is_err());

        assert!(connector.disconnect(connectee_addr).await);
        assert!(connector.connect(connectee_addr).await.is_err());
    }
}

#[tokio::test]
async fn node_creation_used_port_fails() {
    let config = Config {
        listener_addr: Some("127.0.0.1:9".parse().unwrap()), // the official Discard Protocol port
        ..Default::default()
    };
    let node = Node::new(config);

    assert!(node.toggle_listener().await.is_err());
}

#[tokio::test]
async fn node_connect_and_disconnect() {
    let nodes = Arc::new(common::start_test_nodes(2).await);
    connect_nodes(&nodes, Topology::Line).await.unwrap();
    let node1_addr = nodes[1].node().listening_addr().await.unwrap();

    let nodes_clone = nodes.clone();
    deadline!(Duration::from_secs(1), move || nodes_clone
        .iter()
        .all(|n| n.node().num_connected() == 1));

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
async fn node_connecting() {
    let nodes = Arc::new(common::start_test_nodes(2).await);
    let node1_addr = nodes[1].node().listening_addr().await.unwrap();

    assert!(!nodes[0].node().is_connecting(node1_addr));

    let node0 = nodes[0].clone();
    tokio::spawn(async move {
        node0.node().connect(node1_addr).await.unwrap();
    });

    let node0 = nodes[0].clone();
    deadline!(Duration::from_millis(100), move || node0
        .node()
        .is_connecting(node1_addr));

    let nodes_clone = nodes.clone();
    deadline!(Duration::from_secs(1), move || nodes_clone
        .iter()
        .all(|n| n.node().num_connected() == 1));
}

#[tokio::test]
async fn node_self_connection_fails() {
    let node = Node::new(Default::default());
    let own_addr = node.toggle_listener().await.unwrap().unwrap();
    assert!(node.connect(own_addr).await.is_err());
}

#[tokio::test]
async fn node_duplicate_connection_fails() {
    let nodes = common::start_test_nodes(2).await;
    assert!(connect_nodes(&nodes, Topology::Line).await.is_ok());
    assert!(connect_nodes(&nodes, Topology::Line).await.is_err());
}

#[tokio::test]
async fn node_allowed_duplicate_connection_works() {
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
async fn node_two_way_connection_works() {
    let mut nodes = common::start_test_nodes(2).await;
    assert!(connect_nodes(&nodes, Topology::Line).await.is_ok());
    nodes.reverse();
    assert!(connect_nodes(&nodes, Topology::Line).await.is_ok());
}

#[tokio::test]
async fn node_connector_conn_limit_breach_fails() {
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
async fn node_connectee_conn_limit_breach_fails() {
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
    deadline!(Duration::from_secs(1), move || connectee.num_connected()
        == 0);
}

#[tokio::test]
async fn node_connector_per_ip_conn_limit_breach_fails() {
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
    deadline!(Duration::from_secs(1), move || connectee.num_connected()
        == 1);
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
    let server_clone = server.clone();
    deadline!(Duration::from_millis(500), move || server_clone
        .num_connected()
        == 1);

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
        deadline!(Duration::from_millis(500), move || server_clone
            .num_connected()
            == 2);

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
        deadline!(Duration::from_millis(500), move || server.num_connected()
            == 1);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn node_overlapping_duplicate_connection_attempts_fail() {
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
                err_count_clone.fetch_add(1, Relaxed);
            }
        });
    }

    deadline!(Duration::from_secs(1), move || err_count.load(Relaxed)
        == NUM_ATTEMPTS - 1);
}

#[tokio::test]
async fn node_shutdown_closes_the_listener() {
    let node = Node::new(Default::default());
    let addr = node.toggle_listener().await.unwrap().unwrap();

    assert!(TcpListener::bind(addr).await.is_err());
    node.shut_down().await;
    sleep(Duration::from_millis(100)).await; // the CI needs a delay
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
    let target_addr = target_node.node().toggle_listener().await.unwrap().unwrap();

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
    conn_tasks.join_all().await;

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
