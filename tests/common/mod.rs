use tokio::io::AsyncReadExt;

use pea2pea::{
    ConnectionReader, ContainsNode, MessagingProtocol, Node, NodeConfig, PacketingProtocol,
};

use std::{convert::TryInto, net::SocketAddr, ops::Deref, sync::Arc};

#[derive(Clone)]
pub struct GenericNode(Arc<Node>);

impl GenericNode {
    #[allow(dead_code)]
    pub async fn new() -> Arc<Self> {
        let mut config = NodeConfig::default();
        config.name = Some("reader".into());
        Arc::new(Self(Node::new(Some(config)).await.unwrap()))
    }
}

impl Deref for GenericNode {
    type Target = Arc<Node>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ContainsNode for GenericNode {
    fn node(&self) -> &Arc<Node> {
        &self.0
    }
}

#[macro_export]
macro_rules! impl_messaging_protocol {
    ($target: ty) => {
        #[async_trait::async_trait]
        impl MessagingProtocol for $target {
            type Message = ();

            async fn read_message(
                connection_reader: &mut ConnectionReader,
            ) -> std::io::Result<Vec<u8>> {
                // expecting the test messages to be prefixed with their length encoded as a LE u16
                let msg_len_size: usize = 2;

                let buffer = &mut connection_reader.buffer;
                connection_reader
                    .reader
                    .read_exact(&mut buffer[..msg_len_size])
                    .await?;
                let msg_len =
                    u16::from_le_bytes(buffer[..msg_len_size].try_into().unwrap()) as usize;
                connection_reader
                    .reader
                    .read_exact(&mut buffer[..msg_len])
                    .await?;

                Ok(buffer[..msg_len].to_vec())
            }

            fn parse_message(_source: SocketAddr, _: &[u8]) -> Option<Self::Message> {
                Some(())
            }
        }
    };
}

impl_messaging_protocol!(GenericNode);

impl PacketingProtocol for GenericNode {}
