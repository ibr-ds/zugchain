use std::{borrow::Cow, fmt::Display, mem};

use bytes::Bytes;
use prost::Message as _;
use ring_compat::digest::Sha256;
use serde::{Deserialize, Serialize};
use themis_core::{
    net::{DisplayBytes, Message},
    protocol::{Checkpoint, Framework},
};

use crate::app::config;
use rc_blockchain::{
    chainproto::{Block, BlockData, BlockHeader},
    export::{CurrentCheckpoint, Delete, Transaction},
    storage::Storage,
};

#[derive(Debug)]
pub struct BlockCutter {
    config: config::Config,
    buffer: Vec<Transaction>,
}

impl BlockCutter {
    pub fn new(config: config::Config) -> Self {
        Self {
            config,
            buffer: Vec::new(),
        }
    }

    pub fn state(&self) -> CutterState<'_> {
        CutterState {
            buffer: Cow::Borrowed(&self.buffer),
        }
    }

    pub fn apply_state(&mut self, state: CutterState<'_>) {
        self.buffer = state.buffer.into_owned()
    }

    // fn total_size(&self) -> usize {
    //     todo!()
    // }

    pub fn append(&mut self, transaction: Transaction) -> Option<Vec<Transaction>> {
        self.buffer.push(transaction);

        if self.buffer.len() >= self.config.max_block_messages {
            Some(self.cut())
        } else {
            None
        }
    }

    fn cut(&mut self) -> Vec<Transaction> {
        mem::take(&mut self.buffer)
    }

    // pub fn buffer_len(&self) -> usize {
    //     self.buffer.len()
    // }

    // pub fn is_buffer_empty(&self) -> bool {
    //     self.buffer_len() == 0
    // }
}

type Hash = Vec<u8>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockDesc {
    pub block_num: u64,
    pub digest: Hash,
}

impl Display for BlockDesc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(BlocKDesc))
            .field("block_num", &self.block_num)
            .field("digest", &DisplayBytes(&self.digest))
            .finish()
    }
}

impl BlockDesc {
    pub fn new(block_num: u64, digest: Hash) -> Self {
        Self { block_num, digest }
    }
}

#[derive(Debug)]
pub struct Blockchain<T> {
    storage: T,
}

impl<T> Blockchain<T>
where
    T: Storage,
{
    pub fn new(storage: T) -> Self {
        let s = Self { storage };
        tracing::info!(head=%s.head(), "blockchain");
        s
    }

    pub fn checkpoint(&self) -> &CurrentCheckpoint {
        self.storage.get_proof()
    }

    pub fn store_proof(&mut self, sequence: u64, quorum: Vec<Message<Checkpoint>>) {
        let quorum = quorum
            .into_iter()
            .map(|m| {
                let m = m.pack::<Framework>().unwrap();
                rmp_serde::to_vec(&m).unwrap()
            })
            .collect();
        let cp = CurrentCheckpoint {
            sequence,
            messages: quorum,
        };
        self.storage.store_proof(cp);
    }

    pub fn state(&self) -> BlockchainState<'_> {
        BlockchainState {
            head: Cow::Borrowed(self.storage.head()),
        }
    }

    pub fn create_block(&mut self, transaction: Vec<Transaction>) {
        tracing::trace_span!(target: "__measure::app::block", parent: None, "new", number=self.storage.head().number+1);
        let data = transaction
            .into_iter()
            .map(|e| {
                let mut buf = Vec::with_capacity(e.encoded_len());
                e.encode(&mut buf)?;
                Ok(buf.into())
            })
            .collect::<Result<_, prost::EncodeError>>()
            .unwrap();
        let block_data = BlockData { data };

        let previous_digest = Self::hash_header(self.storage.head());
        let next_number = self.storage.head().number + 1;

        let header = BlockHeader {
            number: next_number,
            previous_hash: previous_digest,
            data_hash: Self::hash_data(&block_data),
        };

        tracing::debug!("Created: {}", header);

        let block = Block {
            header: Some(header),
            data: Some(block_data),
            metadata: None,
        };

        self.storage.store_block(block)
    }

    fn hash_data(payload: &BlockData) -> Hash {
        let digest = rc_blockchain::hashing::hash_data::<Sha256>(payload);
        digest.as_slice().to_owned()
    }

    fn hash_header(header: &BlockHeader) -> Hash {
        let digest = rc_blockchain::hashing::hash_header::<Sha256>(header);
        digest.as_slice().to_owned()
    }

    pub fn prune(&mut self, new_base: Delete) {
        tracing::trace!("Deleting blocks from {}", new_base.number());
        self.storage.prune_chain(new_base);
    }

    pub fn get_block(&self, number: u64) -> Option<Block> {
        self.storage.read_block(number)
    }

    pub fn get_block_binary(&self, number: u64) -> Option<Bytes> {
        self.storage.read_block_binary(number).map(Into::into)
    }

    pub fn get_block_header(&self, number: u64) -> Option<BlockHeader> {
        self.storage.read_header(number)
    }

    pub fn overwrite_block(&mut self, block: Block) {
        self.storage.store_block(block)
    }

    pub fn head(&self) -> &BlockHeader {
        self.storage.head()
    }

    pub fn rebase(&mut self, base: Delete, blocks: impl Iterator<Item = Block>) {
        self.storage.prune_chain(base);
        for block in blocks {
            self.storage.store_block(block);
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlockchainState<'a> {
    pub head: Cow<'a, BlockHeader>,
}

impl<'a> BlockchainState<'a> {
    pub fn into_owned(self) -> BlockchainState<'static> {
        BlockchainState {
            head: Cow::Owned(self.head.into_owned()),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CutterState<'a> {
    pub buffer: Cow<'a, [Transaction]>,
}

impl<'a> CutterState<'a> {
    pub fn into_owned(self) -> CutterState<'static> {
        CutterState {
            buffer: Cow::Owned(self.buffer.into_owned()),
        }
    }
}
