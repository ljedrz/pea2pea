use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
};

mod common;
use pea2pea::{
    connect_nodes,
    protocols::{Handshaking, Reading, ReturnableConnection, Writing},
    Node, NodeConfig, Pea2Pea, Topology,
};

use std::{
    io,
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
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
        ..Default::default()
    };
    let _node = Node::new(Some(config)).await.unwrap();
}

#[tokio::test]
async fn node_creation_used_port_fails() {
    let config = NodeConfig {
        desired_listening_port: Some(9), // the official Discard Protocol port
        allow_random_port: false,
        ..Default::default()
    };
    assert!(Node::new(Some(config)).await.is_err());
}

#[tokio::test]
async fn node_connect_and_disconnect() {
    let nodes = common::start_inert_nodes(2, None).await;
    connect_nodes(&nodes, Topology::Line).await.unwrap();

    assert!(nodes[0].disconnect(nodes[1].listening_addr()));
    assert!(!nodes[0].is_connected(nodes[1].listening_addr()));
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
                err_count_clone.fetch_add(1, Ordering::Relaxed);
            }
        });
    }

    wait_until!(1, err_count.load(Ordering::Relaxed) == NUM_ATTEMPTS - 1);
}

#[tokio::test]
async fn node_shutdown_closes_the_listener() {
    let node = Node::new(None).await.unwrap();
    let addr = node.listening_addr();

    assert!(TcpListener::bind(addr).await.is_err());
    node.shut_down();
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

    // a badly implemented handshake protocol; it expects to receive 1B, but doesn't provide it
    impl Handshaking for Wrap {
        fn enable_handshaking(&self) {
            let (from_node_sender, mut from_node_receiver) =
                mpsc::channel::<ReturnableConnection>(1);

            let handshaking_task = tokio::spawn(async move {
                loop {
                    if let Some((mut conn, _)) = from_node_receiver.recv().await {
                        match conn.reader().read_exact(&mut [0u8; 1]).await {
                            Ok(_) => {}
                            Err(_) => unreachable!(),
                        }

                        unreachable!();
                    }
                }
            });

            self.node()
                .set_handshake_handler((from_node_sender, handshaking_task).into());
        }
    }

    let config = NodeConfig {
        max_protocol_setup_time_ms: 10,
        ..Default::default()
    };
    let connector = Wrap(Node::new(None).await.unwrap());
    let connectee = Wrap(Node::new(Some(config)).await.unwrap());

    connector.enable_handshaking();
    connectee.enable_handshaking();

    // the connection attempt should time out...
    assert!(connector
        .node()
        .connect(connectee.node().listening_addr())
        .await
        .is_err());
    // ...but make sure that the connectee has acknowledged that connection attempt
    assert!(!connectee.node().known_peers().read().is_empty());
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

        fn read_message(
            &self,
            _source: SocketAddr,
            buffer: &[u8],
        ) -> io::Result<Option<(Self::Message, usize)>> {
            if buffer.len() >= 2 {
                Ok(Some(((), 2)))
            } else {
                Ok(None)
            }
        }
    }

    let reader = Wrap(Node::new(None).await.unwrap());
    reader.enable_reading();

    // no need to set up a writer node
    let mut writer = TcpStream::connect(reader.node().listening_addr())
        .await
        .unwrap();
    writer.write_all(&[0; 10]).await.unwrap();

    wait_until!(1, reader.node().stats().received() == (5, 10));
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
        .await
        .unwrap();
    reader.read_exact(&mut reader_buf).await.unwrap();
    writer
        .node()
        .send_direct_message(reader_addr, b"derp"[..].into())
        .await
        .unwrap();
    reader.read_exact(&mut reader_buf).await.unwrap();

    wait_until!(1, writer.node().stats().sent() == (2, 8));
}
