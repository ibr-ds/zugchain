use crate::{
    chainproto::{Block, BlockData, BlockHeader},
    display::DisplayBytes,
};
use digest::Digest;

/// FIXME: hlf is doing ANS.1 encoding here, do we care?
pub fn hash_header<D>(header: &BlockHeader) -> digest::Output<D>
where
    D: Digest,
{
    let mut d = D::new();

    d.update(&header.number.to_le_bytes());
    d.update(&header.previous_hash);
    d.update(&header.data_hash);

    d.finalize()
}

/// FIXME In HLF this is a merkle tree
pub fn hash_data<D>(data: &BlockData) -> digest::Output<D>
where
    D: Digest,
{
    let BlockData { data } = data;

    let mut cx = D::new();
    cx.update(&(data.len() as u64).to_le_bytes());
    for tx in data {
        cx.update(tx);
    }

    cx.finalize()
}

pub fn verify_block<D>(expected_previous: &[u8], block: &Block) -> bool
where
    D: Digest,
{
    let data = none_return!(&block.data, false);
    let header = none_return!(&block.header, false);

    if header.previous_hash != expected_previous {
        tracing::warn!(
            "Block previous {} != expected previous {}",
            DisplayBytes(&header.previous_hash),
            DisplayBytes(expected_previous)
        );
        return false;
    };

    let data_hash = hash_data::<D>(data);
    if header.data_hash.as_slice() != data_hash.as_slice() {
        tracing::warn!(expected=%DisplayBytes(&header.data_hash), calculated=%DisplayBytes(&data_hash), "verify data");
        false
    } else {
        true
    }
}

pub fn verify_chain<'a, D, I>(previous_hash: &[u8], blocks: I) -> bool
where
    D: Digest,
    I: IntoIterator<Item = &'a Block>,
{
    let mut blocks = blocks.into_iter();

    let mut prev_block = match blocks.next() {
        Some(block) if verify_block::<D>(previous_hash, block) => block,
        Some(block) => {
            tracing::warn!("Block {} not verified.", block.header.as_ref().unwrap());
            return false;
        }
        None => {
            tracing::warn!("empty chain");
            return false;
        }
    };

    for block in blocks {
        let prev_header = prev_block.header.as_ref().unwrap();
        let previous_hash = hash_header::<D>(prev_header);
        if verify_block::<D>(&previous_hash, block) {
            prev_block = block;
        } else {
            tracing::warn!("{} not verified.", block.header.as_ref().unwrap());
            return false;
        }
    }
    true
}

pub fn verify_chain2<'a, D, I>(previous_hash: &[u8], blocks: I) -> bool
where
    D: Digest,
    I: IntoIterator<Item = &'a Block>,
{
    let blocks: Vec<_> = blocks.into_iter().collect();

    if let Some(first) = blocks.first() {
        if !verify_block::<D>(previous_hash, first) {
            return false;
        }
    }

    for window in blocks.windows(2) {
        let previous = window[0];
        let block = window[1];
        let previous_hash = hash_header::<D>(previous.header.as_ref().unwrap());
        if !verify_block::<D>(&previous_hash, block) {
            return false;
        }
    }
    true
}

pub fn verify_headers<'a, D, I>(previous_hash: &[u8], headers: I) -> bool
where
    D: Digest,
    I: IntoIterator<Item = &'a BlockHeader>,
{
    let headers: Vec<_> = headers.into_iter().collect();

    if let Some(first) = headers.first() {
        if first.previous_hash != previous_hash {
            return false;
        }
    }

    for window in headers.windows(2) {
        let previous = window[0];
        let block = window[1];
        let previous_hash = hash_header::<D>(previous);
        if block.previous_hash.as_slice() != previous_hash.as_slice() {
            return false;
        }
    }
    true
}
