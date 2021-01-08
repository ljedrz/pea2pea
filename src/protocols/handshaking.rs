use crate::Pea2Pea;

/// Can be used to specify and enable network handshakes. Upon establishing a connection, both sides will
/// need to adhere to the specified handshake rules in order to finalize the connection and be able to send
/// or receive any messages.
pub trait Handshaking: Pea2Pea {
    /// Prepares the node to perform specified network handshakes.
    fn enable_handshaking(&self);
}
