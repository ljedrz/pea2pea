use crate::{ReadProtocol, WriteProtocol};

use std::{io, net::SocketAddr, sync::Arc};

pub trait ResponseProtocol: ReadProtocol + WriteProtocol {
    type Message;

    // prepare the Node to act on incoming messages
    fn enable_response_protocol(self: &Arc<Self>);

    fn parse_message(&self, buffer: &[u8]) -> Option<Self::Message>;

    fn process_message(&self, _message: &Self::Message) {
        // do nothing by default
    }

    fn respond_to_message(
        self: &Arc<Self>,
        _message: Self::Message,
        _source: SocketAddr,
    ) -> io::Result<()> {
        // don't do anything by default
        Ok(())
    }
}
