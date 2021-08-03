use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::sleep,
};

mod common;
use pea2pea::{
    connect_nodes,
    protocols::{Handshaking, Reading, Writing},
    Connection, Node, NodeConfig, Pea2Pea, Topology,
};

use std::{
    io,
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering::Relaxed},
        Arc,
    },
    time::Duration,
};

#[tokio::test]
async fn node_creation_any_port_works() {
    let _node = Node::new(None).await.unwrap();
}

#[should_panic]
#[tokio::test]
async fn node_creation_bad_params_panic() {
    let config = NodeConfig {
        allow_random_port: false,
        listener_ip: "127.0.0.1".parse().unwrap(),
        ..Default::default()
    };
    let _node = Node::new(Some(config)).await.unwrap();
}

#[tokio::test]
async fn node_creation_used_port_fails() {
    let config = NodeConfig {
        desired_listening_port: Some(9), // the official Discard Protocol port
        allow_random_port: false,
        listener_ip: "127.0.0.1".parse().unwrap(),
        ..Default::default()
    };
    assert!(Node::new(Some(config)).await.is_err());
}

#[tokio::test]
async fn node_connect_and_disconnect() {
    let nodes = common::start_inert_nodes(2, None).await;
    connect_nodes(&nodes, Topology::Line).await.unwrap();

    wait_until!(
        1,
        nodes[0].num_connected() == 1 && nodes[1].num_connected() == 1
    );

    assert!(nodes[0].num_connecting() == 0);
    assert!(nodes[1].num_connecting() == 0);

    assert!(nodes[0].disconnect(nodes[1].listening_addr()));

    wait_until!(1, nodes[0].num_connected() == 0);

    // node[1] didn't enable reading, so it has no way of knowing
    // that the connection has been broken by node[0]
    assert_eq!(nodes[1].num_connected(), 1);
}

#[tokio::test]
async fn node_self_connection_fails() {
    let node = Node::new(None).await.unwrap();
    assert!(node.connect(node.listening_addr()).await.is_err());
}

#[tokio::test]
async fn node_duplicate_connection_fails() {
    let nodes = common::start_inert_nodes(2, None).await;
    assert!(connect_nodes(&nodes, Topology::Line).await.is_ok());
    assert!(connect_nodes(&nodes, Topology::Line).await.is_err());
}

#[tokio::test]
async fn node_connector_limit_breach_fails() {
    let config = NodeConfig {
        max_connections: 0,
        listener_ip: "127.0.0.1".parse().unwrap(),
        ..Default::default()
    };
    let connector = Node::new(Some(config)).await.unwrap();
    let connectee = Node::new(None).await.unwrap();

    assert!(connector.connect(connectee.listening_addr()).await.is_err());
}

#[tokio::test]
async fn node_connectee_limit_breach_fails() {
    let config = NodeConfig {
        max_connections: 0,
        listener_ip: "127.0.0.1".parse().unwrap(),
        ..Default::default()
    };
    let connectee = Node::new(Some(config)).await.unwrap();
    let connector = Node::new(None).await.unwrap();

    // a breached connection limit doesn't close the listener, so this works
    connector.connect(connectee.listening_addr()).await.unwrap();

    // the number of connections on connectee side needs to be checked instead
    wait_until!(1, connectee.num_connected() == 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn node_overlapping_duplicate_connection_attempts_fail() {
    const NUM_ATTEMPTS: usize = 5;

    let connector = Node::new(None).await.unwrap();
    let connectee = Node::new(None).await.unwrap();
    let addr = connectee.listening_addr();

    let err_count = Arc::new(AtomicUsize::new(0));
    for _ in 0..NUM_ATTEMPTS {
        let connector_clone = connector.clone();
        let err_count_clone = err_count.clone();
        tokio::spawn(async move {
            if connector_clone.connect(addr).await.is_err() {
                err_count_clone.fetch_add(1, Relaxed);
            }
        });
    }

    wait_until!(1, err_count.load(Relaxed) == NUM_ATTEMPTS - 1);
}

#[tokio::test]
async fn node_shutdown_closes_the_listener() {
    let node = Node::new(None).await.unwrap();
    let addr = node.listening_addr();

    assert!(TcpListener::bind(addr).await.is_err());
    node.shut_down();
    sleep(Duration::from_millis(100)).await;
    assert!(TcpListener::bind(addr).await.is_ok());
}

#[tokio::test]
async fn node_hung_handshake_fails() {
    #[derive(Clone)]
    struct Wrap(Node);

    impl Pea2Pea for Wrap {
        fn node(&self) -> &Node {
            &self.0
        }
    }

    // a badly implemented handshake protocol; 1B is expected by both the initiator and the responder (no distinction
    // is even made), but it is never provided by either of them
    #[async_trait::async_trait]
    impl Handshaking for Wrap {
        async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
            let _ = conn.reader().read_exact(&mut [0u8; 1]).await;

            unreachable!();
        }
    }

    let config = NodeConfig {
        max_handshake_time_ms: 10,
        listener_ip: "127.0.0.1".parse().unwrap(),
        ..Default::default()
    };
    let connector = Wrap(Node::new(None).await.unwrap());
    let connectee = Wrap(Node::new(Some(config)).await.unwrap());

    // note: the connector does NOT enable handshaking
    connectee.enable_handshaking();

    // the connection attempt should register just fine for the connector, as it doesn't expect a handshake
    assert!(connector
        .node()
        .connect(connectee.node().listening_addr())
        .await
        .is_ok());

    // the TPC connection itself has been established, and with no reading, the connector doesn't know
    // that the connectee has already disconnected from it by now
    assert!(connector.node().num_connected() == 1);
    assert!(connector.node().num_connecting() == 0);

    // the connectee should have rejected the connection attempt on its side
    assert!(connectee.node().num_connected() == 0);
    assert!(connectee.node().num_connecting() == 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn node_common_timeout_when_spammed_with_connections() {
    const NUM_ATTEMPTS: u16 = 200;
    const TIMEOUT_SECS: u64 = 1;

    #[derive(Clone)]
    struct Wrap(Node);

    impl Pea2Pea for Wrap {
        fn node(&self) -> &Node {
            &self.0
        }
    }

    #[async_trait::async_trait]
    impl Handshaking for Wrap {
        async fn perform_handshake(&self, mut conn: Connection) -> io::Result<Connection> {
            conn.reader().read_exact(&mut [0u8; 1]).await?;

            Ok(conn)
        }
    }

    let config = NodeConfig {
        max_handshake_time_ms: TIMEOUT_SECS * 1_000,
        max_connections: NUM_ATTEMPTS,
        listener_ip: "127.0.0.1".parse().unwrap(),
        ..Default::default()
    };
    let victim = Wrap(Node::new(Some(config)).await.unwrap());
    victim.enable_handshaking();
    let victim_addr = victim.node().listening_addr();

    let mut sockets = Vec::with_capacity(NUM_ATTEMPTS as usize);

    for _ in 0..NUM_ATTEMPTS {
        if let Ok(socket) = TcpStream::connect(victim_addr).await {
            sockets.push(socket);
        }
    }

    wait_until!(3, victim.node().num_connecting() == NUM_ATTEMPTS as usize);

    wait_until!(TIMEOUT_SECS + 1, victim.node().num_connecting() == 0);
}

#[tokio::test]
async fn node_stats_received() {
    #[derive(Clone)]
    struct Wrap(Node);

    impl Pea2Pea for Wrap {
        fn node(&self) -> &Node {
            &self.0
        }
    }

    // a trivial protocol with fixed-length 2B messages
    impl Reading for Wrap {
        type Message = ();

        fn read_message(&self, _src: SocketAddr, buffer: &[u8]) -> io::Result<Option<((), usize)>> {
            if buffer.len() >= 2 {
                Ok(Some(((), 2)))
            } else {
                Ok(None)
            }
        }
    }

    let reader = Wrap(Node::new(None).await.unwrap());
    reader.enable_reading();

    // no need to set up a writer node; a raw stream will suffice
    let mut writer = TcpStream::connect(reader.node().listening_addr())
        .await
        .unwrap();
    let writer_addr = writer.local_addr().unwrap();
    writer.write_all(&[0; 10]).await.unwrap();

    wait_until!(1, reader.node().stats().received() == (5, 10));
    wait_until!(1, {
        if let Some(peer) = reader.node().known_peers().read().get(&writer_addr) {
            peer.msgs_received.load(Relaxed) == 5 && peer.bytes_received.load(Relaxed) == 10
        } else {
            false
        }
    });
}

#[tokio::test]
async fn node_stats_sent() {
    #[derive(Clone)]
    struct Wrap(Node);

    impl Pea2Pea for Wrap {
        fn node(&self) -> &Node {
            &self.0
        }
    }

    // a trivial writing protocol
    impl Writing for Wrap {
        fn write_message(
            &self,
            _: SocketAddr,
            payload: &[u8],
            buffer: &mut [u8],
        ) -> io::Result<usize> {
            buffer[..payload.len()].copy_from_slice(&payload);
            Ok(payload.len())
        }
    }

    let writer = Wrap(Node::new(None).await.unwrap());
    writer.enable_writing();

    // no need to set up a reader node
    let listener = TcpListener::bind("0.0.0.0:0".parse::<SocketAddr>().unwrap())
        .await
        .unwrap();
    let reader_addr = listener.local_addr().unwrap();
    let listener_task = tokio::spawn(async move { listener.accept().await.unwrap() });

    writer.node().connect(reader_addr).await.unwrap();
    let (mut reader, _) = listener_task.await.unwrap();
    let mut reader_buf = [0u8; 4];

    writer
        .node()
        .send_direct_message(reader_addr, b"herp"[..].into())
        .unwrap();
    reader.read_exact(&mut reader_buf).await.unwrap();
    writer
        .node()
        .send_direct_message(reader_addr, b"derp"[..].into())
        .unwrap();
    reader.read_exact(&mut reader_buf).await.unwrap();

    wait_until!(1, writer.node().stats().sent() == (2, 8));
    wait_until!(1, {
        if let Some(peer) = writer.node().known_peers().read().get(&reader_addr) {
            peer.msgs_sent.load(Relaxed) == 2 && peer.bytes_sent.load(Relaxed) == 8
        } else {
            false
        }
    });
}
