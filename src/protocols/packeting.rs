use crate::ContainsNode;

pub trait PacketingProtocol: ContainsNode {
    fn enable_packeting_protocol(&self);
}

pub type PacketingClosure = Box<dyn Fn(&mut Vec<u8>) + Send + Sync>;
