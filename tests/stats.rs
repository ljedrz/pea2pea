use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

mod common;
use pea2pea::{
    protocols::{Reading, Writing},
    Node, Pea2Pea,
};

use std::{io, net::SocketAddr, sync::atomic::Ordering::Relaxed};

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
    let mut writer = TcpStream::connect(reader.node().listening_addr().unwrap())
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
            buffer[..payload.len()].copy_from_slice(payload);
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
