use std::sync::Arc;

pub trait BroadcastProtocol {
    const INTERVAL_MS: u64;

    // prepare the Node to broadcast messages
    fn enable_broadcast_protocol(self: &Arc<Self>);
}
