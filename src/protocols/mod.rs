//! Opt-in protocols available to the `Node`.

use crate::connections::Connection;

use once_cell::sync::OnceCell;
use tokio::sync::oneshot;

use std::io;

mod handshaking;
mod reading;
mod writing;

pub use handshaking::{HandshakeHandler, Handshaking};
pub use reading::{Reading, ReadingHandler};
pub use writing::{Writing, WritingHandler};

#[derive(Default)]
pub(crate) struct Protocols {
    pub(crate) handshake_handler: OnceCell<HandshakeHandler>,
    pub(crate) reading_handler: OnceCell<ReadingHandler>,
    pub(crate) writing_handler: OnceCell<WritingHandler>,
}

/// An object allowing a `Connection` to be "borrowed" from the owning `Node` to enable a protocol
/// and to be sent back to it once it's done its job.
pub type ReturnableConnection = (Connection, oneshot::Sender<io::Result<Connection>>);
