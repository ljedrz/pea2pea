use crate::ContainsNode;

use tokio::time::sleep;
use tracing::*;

use std::{io, time::Duration};

#[async_trait::async_trait]
pub trait BroadcastProtocol: ContainsNode
where
    Self: Clone + Send + Sync + 'static,
{
    const INTERVAL_MS: u64;

    // prepare the Node to broadcast messages
    fn enable_broadcast_protocol(&self) {
        let self_clone = self.clone();
        tokio::spawn(async move {
            loop {
                if let Err(e) = self_clone.perform_broadcast().await {
                    error!(parent: self_clone.node().span(), "broadcast failed: {}", e);
                }

                sleep(Duration::from_millis(Self::INTERVAL_MS)).await;
            }
        });
    }

    async fn perform_broadcast(&self) -> io::Result<()>;
}
