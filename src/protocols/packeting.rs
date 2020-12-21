use crate::ContainsNode;

pub trait PacketingProtocol: ContainsNode {
    fn enable_packeting_protocol(&self, packeting_closure: PacketingClosure) {
        self.node()
            .set_packeting_closure(Box::new(packeting_closure));
    }
}

pub type PacketingClosure = Box<dyn Fn(&[u8]) -> Vec<u8> + Send + Sync + 'static>;
