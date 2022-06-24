use crate::{
    chainproto::{Block, BlockHeader},
    export::{CurrentCheckpoint, Delete},
};
use enum_dispatch::enum_dispatch;

#[enum_dispatch]
pub trait Storage: Send {
    fn store_block(&mut self, block: Block);

    fn store_proof(&mut self, _proof: CurrentCheckpoint);
    fn get_proof(&self) -> &CurrentCheckpoint;

    /// Assumes delete has a verified signature
    fn prune_chain(&mut self, delete: Delete);

    fn base(&self) -> &Delete;
    fn head(&self) -> &BlockHeader;

    fn read_header(&self, index: u64) -> Option<BlockHeader> {
        self.read_block(index).and_then(|b| b.header)
    }
    fn read_block(&self, index: u64) -> Option<Block>;
    fn read_block_binary(&self, index: u64) -> Option<Vec<u8>> {
        let _ = index;
        None
    }
}

mod files;
pub use files::*;
mod memory;
pub use memory::*;

#[enum_dispatch(Storage)]
pub enum Storages {
    Memory(InMemory),
    Files(Files),
}
