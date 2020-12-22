use crate::ContainsNode;

use tokio::time::sleep;
use tracing::*;

use std::{io, time::Duration};

#[async_trait::async_trait]
pub trait MaintenanceProtocol: ContainsNode
where
    Self: Clone + Send + Sync + 'static,
{
    const INTERVAL_MS: u64;

    // prepare the Node to run a maintenance routine
    fn enable_maintenance_protocol(&self) {
        let self_clone = self.clone();
        tokio::spawn(async move {
            loop {
                if let Err(e) = self_clone.perform_maintenance().await {
                    error!(parent: self_clone.node().span(), "maintenance failed: {}", e);
                }

                sleep(Duration::from_millis(Self::INTERVAL_MS)).await;
            }
        });
    }

    async fn perform_maintenance(&self) -> io::Result<()>;
}
