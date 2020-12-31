//! Opt-in protocols available to the `Node`.

use once_cell::sync::OnceCell;

mod handshaking;
mod reading;
mod writing;

pub use handshaking::{HandshakeHandler, HandshakeObjects, Handshaking};
pub use reading::{Reading, ReadingHandler, ReadingObjects};
pub use writing::{Writing, WritingHandler, WritingObjects};

#[derive(Default)]
pub(crate) struct Protocols {
    pub(crate) handshake_handler: OnceCell<HandshakeHandler>,
    pub(crate) reading_handler: OnceCell<ReadingHandler>,
    pub(crate) writing_handler: OnceCell<WritingHandler>,
}
