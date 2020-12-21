use crate::ContainsNode;

pub trait WriteProtocol: ContainsNode {
    fn enable_writing_protocol(&self, writing_closure: WritingClosure) {
        self.node().set_writing_closure(Box::new(writing_closure));
    }
}

pub type WritingClosure = Box<dyn Fn(&[u8]) -> Vec<u8> + Send + Sync + 'static>;
