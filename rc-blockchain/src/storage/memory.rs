use super::Storage;
use crate::{
    chainproto,
    chainproto::Block,
    export::{CurrentCheckpoint, Delete},
    genesis::GENESIS_HEAD,
};

#[derive(Debug, Default)]
pub struct InMemory {
    base: Delete,
    storage: Vec<Block>,
    checkpoint: CurrentCheckpoint,
}

impl InMemory {
    pub fn new() -> Self {
        let header = GENESIS_HEAD;
        let base = Delete {
            header: Some(header),
            signature: Vec::default(),
        };
        Self {
            base,
            storage: Vec::default(),
            checkpoint: Default::default(),
        }
    }

    #[allow(unused)]
    fn dump(&self) {
        tracing::trace!("Expected start: {}", self.offset());
        for block in &self.storage {
            tracing::trace!("{}", block.header.as_ref().unwrap());
        }
    }

    fn offset(&self) -> u64 {
        self.base.header.as_ref().unwrap().number + 1
    }
}

impl Storage for InMemory {
    fn store_block(&mut self, block: Block) {
        tracing::trace!(number = block.number(), "store_block");
        self.storage.push(block)
    }

    fn prune_chain(&mut self, delete: Delete) {
        let old_offset = self.offset();
        self.base = delete;
        let delete_until = self.offset() - 1;
        tracing::trace!("Deleting including {}", delete_until);
        for block in self.storage.drain(..=(delete_until - old_offset) as usize) {
            tracing::trace!("Removing {:?}", block.header.map(|h| h.number));
        }
    }

    fn read_block(&self, index: u64) -> Option<Block> {
        if index < self.offset() {
            return None;
        }
        self.storage.get((index - self.offset()) as usize).cloned()
    }

    fn read_header(&self, index: u64) -> Option<chainproto::BlockHeader> {
        if index < self.offset() {
            return None;
        }
        self.storage
            .get((index - self.offset()) as usize)
            .and_then(|b| b.header.clone())
    }

    fn base(&self) -> &Delete {
        &self.base
    }

    fn head(&self) -> &chainproto::BlockHeader {
        if let Some(block) = self.storage.last() {
            block.header()
        } else {
            self.base.header()
        }
    }

    fn store_proof(&mut self, proof: crate::export::CurrentCheckpoint) {
        self.checkpoint = proof;
    }

    fn get_proof(&self) -> &crate::export::CurrentCheckpoint {
        &self.checkpoint
    }
}
