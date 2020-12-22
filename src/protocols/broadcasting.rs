pub trait BroadcastProtocol
where
    Self: Clone,
{
    const INTERVAL_MS: u64;

    // prepare the Node to broadcast messages
    fn enable_broadcast_protocol(&self);
}
