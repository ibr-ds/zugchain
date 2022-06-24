use digest::{Digest, Output};

use crate::{
    chainproto::{Block, BlockData, BlockHeader, BlockMetadata},
    hashing::hash_header,
};

pub const FIRST_BLOCK_NUM: u64 = GENESIS_NUMBER + 1;

pub const GENESIS_NUMBER: u64 = 0;

pub const GENESIS_HEAD: BlockHeader = BlockHeader {
    number: GENESIS_NUMBER,
    previous_hash: Vec::new(),
    data_hash: Vec::new(),
};

pub fn genesis_hash<D>() -> Output<D>
where
    D: Digest,
{
    hash_header::<D>(&GENESIS_HEAD)
}

pub fn genesis_block() -> Block {
    let header = GENESIS_HEAD;
    Block {
        header: Some(header),
        data: Some(BlockData::default()),
        metadata: Some(BlockMetadata {
            metadata: Default::default(),
        }),
    }
}
