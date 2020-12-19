use std::{io, net::SocketAddr, sync::Arc};

pub trait ResponseProtocol {
    type Message;

    // prepare the Node to act on incoming messages
    fn enable_response_protocol(self: &Arc<Self>);

    fn parse_message(&self, buffer: &[u8]) -> Option<Self::Message>;

    fn validate_message(&self, message: &Self::Message) -> bool {
        // accept the message by default
        true
    }

    fn process_message(
        self: &Arc<Self>,
        message: Self::Message,
        source: SocketAddr,
    ) -> io::Result<()> {
        // don't do anything by default
        Ok(())
    }
}
