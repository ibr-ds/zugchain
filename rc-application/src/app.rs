use std::{cmp, future::Future};

use bytes::Bytes;
use crypto::Keys;
use prost::Message as _;

use ring_compat::digest::Sha256;
use serde::{Deserialize, Serialize};
use themis_core::{
    app::{Application, ApplyError, Request, RequestFlags, Response, ResponseFlags},
    net::{Message, Sequenced},
    protocol::Checkpoint,
};

use rc_blockchain::{
    chainproto::Block,
    export::{self, Blocks, CheckpointProof, CheckpointedBlocks, Delete, Export, Headers, Read, ReadOp, Transaction},
    genesis::{genesis_hash, GENESIS_HEAD, GENESIS_NUMBER},
    hashing::{hash_header, verify_chain},
    storage::Storage,
};

use crate::app::{
    chain::{BlockCutter, Blockchain},
    BlockchainState, CutterState,
};

pub mod chain;
pub mod config;
pub mod crypto;
pub use chain::*;
pub use config::*;
use tracing::{Instrument, Span};

#[derive(Debug)]
struct ExportOps {
    span: Option<Span>,
}

impl ExportOps {
    fn new() -> Self {
        Self { span: None }
    }

    fn export_start(&mut self, export: &export::Read, hashed: bool) {
        let span = tracing::info_span!(target: "__measure::export", parent: None, "export", base=export.base, hashed);
        self.span = Some(span);
    }

    fn export_finished(&mut self) {
        self.span.take();
    }
}

#[derive(Debug)]
pub struct Railchain<T> {
    id: u64,
    block_cutter: BlockCutter,
    blockchain: Blockchain<T>,
    prune_base: Option<Delete>,

    checkpoint_interval: u64,

    keys: crypto::Keys,

    // measurements
    delete_span: ExportOps,
}

impl<T> Railchain<T>
where
    T: Storage,
{
    pub fn new(
        id: u64,
        checkpoint_interval: u64,
        block_cutter: BlockCutter,
        blockchain: Blockchain<T>,
        keys: Keys,
    ) -> Self {
        Self {
            id,
            checkpoint_interval,
            block_cutter,
            blockchain,
            prune_base: None,
            keys,
            delete_span: ExportOps::new(),
        }
    }

    fn append_transaction(&mut self, transaction: Transaction) {
        tracing::trace!("Buffering Transaction");
        if let Some(block_data) = self.block_cutter.append(transaction) {
            self.blockchain.create_block(block_data);
            crate::metrics::record_block_num(self.blockchain.head().number);
        };
    }

    fn read_blocks(&self, read: Read) -> Blocks {
        let base = self
            .prune_base
            .as_ref()
            .and_then(|b| b.header.as_ref())
            .map(|h| h.number)
            .unwrap_or(0);
        let start = cmp::max(read.base, base);
        let end = self.blockchain.head().number;
        tracing::trace!(start, end, "read_blocks");
        let blocks: Vec<_> = (start..end + 1)
            .filter_map(|index| self.blockchain.get_block(index))
            .collect();

        tracing::info!(start, end, len = blocks.len(), "returning blocks");

        let head_signature = if !blocks.is_empty() {
            self.keys
                .sign_header(blocks.last().as_ref().and_then(|b| b.header.as_ref()).unwrap())
        } else {
            vec![]
        };

        Blocks {
            block: blocks,
            base: self.prune_base.clone(),
            head_signature,
        }
    }

    fn read_block_headers(&mut self, read: Read) -> Headers {
        let base = self
            .prune_base
            .as_ref()
            .and_then(|b| b.header.as_ref())
            .map(|h| h.number)
            .unwrap_or(0);
        let start = cmp::max(read.base, base);
        let end = self.blockchain.head().number;
        let blocks: Vec<_> = (start..end + 1)
            .filter_map(|index| self.blockchain.get_block_header(index))
            .collect();

        tracing::info!(start, end, len = blocks.len(), "returning headers");

        let head_signature = if !blocks.is_empty() {
            self.keys.sign_header(blocks.last().as_ref().unwrap())
        } else {
            vec![]
        };

        Headers {
            header: blocks,
            base: self.prune_base.clone(),
            head_signature,
        }
    }

    fn delete_blocks(&mut self, delete: Delete) {
        let header = delete.header.as_ref().unwrap();

        if !self.keys.verify_header(header, &delete.signature) {
            return;
        }

        tracing::info!("Pruning blocks from {}", header);
        self.blockchain.prune(delete.clone());
        self.prune_base = Some(delete);
    }

    fn export_v2(&mut self, read: Read, flags: RequestFlags) -> themis_core::Result<Vec<u8>> {
        let checkpoint = self.blockchain.checkpoint();
        let cp_head_block = checkpoint.sequence / self.checkpoint_interval;
        tracing::trace!(
            checkpoint = checkpoint.sequence,
            cp_head_block,
            interval = self.checkpoint_interval,
            "exportv2"
        );
        if flags.contains(RequestFlags::HASH_REPLY) {
            let mut cp = CheckpointProof::default();

            let prune_base = self.prune_base.as_ref().map(|b| b.number()).unwrap_or(0);
            let base = read.base.max(prune_base);
            if prune_base > read.base {
                cp.base = self.prune_base.clone();
            }

            if cp_head_block > base {
                let head = self.blockchain.get_block_header(cp_head_block);
                cp.quorum = Some(checkpoint.clone());
                cp.head = head;
            };

            tracing::debug!(?flags, base, head = cp_head_block, "exportv2");
            to_vec(&cp)
        } else {
            let mut cp_proof = CheckpointProof::default();
            let mut cp = CheckpointedBlocks::default();

            let prune_base = self.prune_base.as_ref().map(|b| b.number()).unwrap_or(0);
            let base = read.base.max(prune_base);
            if prune_base > read.base {
                cp_proof.base = self.prune_base.clone();
            }

            if cp_head_block > base {
                tracing::debug!(start = base + 1, end = cp_head_block, "returning blocks");
                let mut blocks = Vec::with_capacity((cp_head_block - base + 1) as usize);
                for i in base + 1..=cp_head_block {
                    tracing::trace!(num = i, "reading block");
                    let block = self.blockchain.get_block_binary(i).unwrap();
                    blocks.push(block);
                }
                cp_proof.head = blocks
                    .last()
                    .map(|b| Block::decode(b.clone()).unwrap())
                    .and_then(|b| b.header);
                cp_proof.quorum = Some(checkpoint.clone());
                cp.blocks = blocks;
            };

            cp.proof = Some(cp_proof);
            tracing::debug!(?flags, base, head = cp_head_block, "exportv2");
            to_vec(&cp)
        }
    }

    fn export_v1(&mut self, read: Read, flags: RequestFlags) -> themis_core::Result<Vec<u8>> {
        if flags.contains(RequestFlags::HASH_REPLY) {
            let headers = self.read_block_headers(read);
            to_vec(&headers)
        } else {
            let blocks = self.read_blocks(read);
            to_vec(&blocks)
        }
    }

    fn export_blocks_only(&mut self, read: Read) -> themis_core::Result<Vec<u8>> {
        let prune_base = self.prune_base.as_ref().map(|b| b.number()).unwrap_or(0);
        let base = read.base.max(prune_base);

        let head = read.head.min(self.blockchain.head().number);

        let mut blocks = Vec::new();
        for i in base + 1..=head {
            let block = self.blockchain.get_block_binary(i).unwrap();
            blocks.push(block);
        }

        let cp = CheckpointedBlocks {
            blocks,
            ..Default::default()
        };
        to_vec(&cp)
    }

    fn handle_export_message(&mut self, flags: RequestFlags, export: Export) -> themis_core::Result<Vec<u8>> {
        match export.command.unwrap() {
            export::export::Command::Read(read) => {
                self.delete_span
                    .export_start(&read, flags.contains(RequestFlags::HASH_REPLY));
                tracing::info!("Exporting Blocks");
                match ReadOp::from_i32(read.version) {
                    Some(ReadOp::V1) => self.export_v1(read, flags),
                    Some(ReadOp::V2) => self.export_v2(read, flags),
                    Some(ReadOp::Read) => self.export_blocks_only(read),
                    None => Ok(Vec::default()),
                }
            }
            export::export::Command::Delete(delete) => {
                self.delete_blocks(delete);
                self.delete_span.export_finished();
                Ok(vec![])
            }
        }
    }
}

fn to_vec<M: prost::Message>(message: &M) -> Result<Vec<u8>, themis_core::Error> {
    let mut buf = Vec::with_capacity(message.encoded_len());
    message
        .encode(&mut buf)
        .map_err(|e| themis_core::Error::application(Box::new(e)))?;
    Ok(buf)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RailchainState<'a> {
    cutter: CutterState<'a>,
    chain: BlockchainState<'a>,
}

impl<'a> RailchainState<'a> {
    pub fn new(cutter: CutterState<'a>, chain: BlockchainState<'a>) -> Self {
        Self { cutter, chain }
    }

    #[allow(unused)]
    fn into_owned(self) -> RailchainState<'static> {
        RailchainState {
            cutter: self.cutter.into_owned(),
            chain: self.chain.into_owned(),
        }
    }
}

impl<'f, T> Application<'f> for Railchain<T>
where
    T: Storage + 'static,
{
    type ExecuteFut = impl Future<Output = themis_core::Result<Message<Response>>> + 'f;

    fn execute(&'f mut self, request: Message<Request>) -> Self::ExecuteFut {
        async move {
            tracing::trace!(target: "__measure::app", parent: None, "execute");
            tracing::info!(
                source = request.source,
                flags = format_args!("{:?}", request.inner.flags),
                ts = request.inner.sequence,
                "request"
            );
            let sequence = request.sequence();
            let flags = request.inner.flags;
            let command = rc_blockchain::export::Command::decode(request.inner.payload)
                .map_err(|e| themis_core::Error::application(Box::new(e)))?;

            let mut response_message = Message::new(self.id, request.source, Response::new(sequence, Bytes::new()));

            match command.action {
                Some(export::command::Action::Export(export)) => {
                    let result = self.handle_export_message(flags, export)?;
                    response_message.inner.payload = result.into();
                    if flags.contains(RequestFlags::HASH_REPLY) {
                        response_message.inner.flags.set(ResponseFlags::HASHED, true);
                    }
                }
                Some(export::command::Action::Transaction(bytes)) => {
                    self.append_transaction(bytes);
                }
                _ => {
                    tracing::warn!("unknown command");
                }
            }

            Ok(response_message)
        }
        .instrument(tracing::trace_span!(target: "__measure::app", parent: None, "execute"))
    }

    type CheckpointHandle = RailchainState<'f>;
    type CheckpointData = FullCheckpointData;

    type TakeFut = impl Future<Output = themis_core::Result<Self::CheckpointHandle>> + 'f;

    fn take_checkpoint(&'f mut self) -> Self::TakeFut {
        async move {
            tracing::trace!(target: "__measure::app", parent: None, "checkpoint");
            tracing::debug!("Taking Checkpoint");
            let state = RailchainState {
                cutter: self.block_cutter.state(),
                chain: self.blockchain.state(),
            };
            tracing::trace!("Buffer contains {} transactions", state.cutter.buffer.len());
            tracing::trace!("Last Block: {}", state.chain.head);
            Ok(state)
        }
        .instrument(tracing::trace_span!(target: "__measure::app::take", parent: None, "take"))
    }

    type ApplyFut = impl Future<Output = Result<(), ApplyError>> + 'f;

    fn apply_checkpoint(
        &'f mut self,
        handle: Self::CheckpointHandle,
        checkpoint: Self::CheckpointData,
    ) -> Self::ApplyFut {
        async move {
            //FIXME signature check on Delete

            let last_header = checkpoint.blocks.last().and_then(|b| b.header.as_ref()).unwrap();

            if handle.chain.head.as_ref() != last_header {
                return Err(ApplyError::Mismatch);
            }

            if let Some(base) = checkpoint.base {
                let base_hash = hash_header::<Sha256>(base.header.as_ref().unwrap());
                if !verify_chain::<Sha256, _>(&base_hash, &checkpoint.blocks) {
                    return Err(ApplyError::Mismatch);
                }

                self.prune_base = Some(base.clone());
                self.blockchain.rebase(base, checkpoint.blocks.into_iter());
            } else {
                if !verify_chain::<Sha256, _>(genesis_hash::<Sha256>().as_slice(), &checkpoint.blocks) {
                    return Err(ApplyError::Mismatch);
                }
                let base = Delete {
                    header: Some(GENESIS_HEAD),
                    signature: Vec::default(),
                };
                self.blockchain.rebase(base, checkpoint.blocks.into_iter());
            }

            self.block_cutter.apply_state(handle.cutter);

            Ok(())
        }
        .instrument(tracing::trace_span!(target: "__measure::app::apply", parent: None, "apply"))
    }

    type ResolveFut = impl Future<Output = themis_core::Result<Self::CheckpointData>> + 'f;

    fn resolve_checkpoint(&'f mut self, handle: Self::CheckpointHandle) -> Self::ResolveFut {
        async move {
            let base = self
                .prune_base
                .as_ref()
                .and_then(|b| b.header.as_ref())
                .map(|h| h.number)
                .unwrap_or(GENESIS_NUMBER);
            let head = handle.chain.head.number;

            tracing::debug!("Resolving checkpoint from {} to {}", base, head);

            let blocks: Option<Vec<Block>> = (base + 1..=head).map(|i| self.blockchain.get_block(i)).collect();
            let blocks = blocks.expect("missing blocks"); //FIXME error handling

            let checkpoint = FullCheckpointData {
                blocks,
                base: self.prune_base.clone(),
            };
            Ok(checkpoint)
        }
        .instrument(tracing::trace_span!(target: "__measure::app::resolve", parent: None, "resolve"))
    }

    fn checkpoint_stable(&mut self, sequence: u64, quorum: Vec<Message<Checkpoint>>) -> themis_core::Result<()> {
        tracing::info!(sequence, "storing checkpoint");
        self.blockchain.store_proof(sequence, quorum);
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FullCheckpointData {
    base: Option<Delete>,
    blocks: Vec<Block>,
}

#[cfg(test)]
mod test {
    use std::borrow::Cow;

    use prost::Message as _;
    use rc_blockchain::{
        chainproto::BlockHeader,
        export::{Blocks, Command, Delete},
        genesis::genesis_hash,
        hashing::verify_chain,
        storage::InMemory,
    };
    use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey};
    use ring_compat::digest::Sha256;
    use themis_core::{
        app::{Application, Request, RequestFlags},
        net::Message,
    };

    use crate::to_vec;

    use super::{
        chain::{BlockCutter, Blockchain, BlockchainState, CutterState},
        config::Export,
        crypto::Keys,
        Config, Railchain, RailchainState,
    };

    fn make_keypair() -> (Ed25519KeyPair, UnparsedPublicKey<Vec<u8>>) {
        let rng = ring::rand::SystemRandom::new();
        let doc = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let key = Ed25519KeyPair::from_pkcs8(doc.as_ref()).unwrap();

        let pk = key.public_key().as_ref().to_vec();
        let pk = UnparsedPublicKey::new(&ring::signature::ED25519, pk);
        (key, pk)
    }

    fn make_app() -> Railchain<InMemory> {
        let config = Config {
            max_block_messages: 1,
            export: Export::default(),
        };

        let keys = make_keypair();
        let keys = Keys::new(keys.0, keys.1);

        let chain = Blockchain::new(InMemory::new());
        let cutter = BlockCutter::new(config);
        Railchain::new(0, 10, cutter, chain, keys)
    }

    fn make_tx() -> Message<Request> {
        let tx = Command::transaction(vec![5, 5, 5].into());
        make_message(tx)
    }

    fn make_message(command: Command) -> Message<Request> {
        use prost::Message as _;

        let mut tx = Vec::with_capacity(command.encoded_len());
        command.encode(&mut tx).unwrap();

        let request = Request::builder()
            .flag(RequestFlags::UNSEQUENCED)
            .payload(tx.into())
            .build();
        Message::new(100, 0, request)
    }

    #[tokio::test]
    async fn checkpoint_one() {
        let _ = tracing_subscriber::fmt::try_init();

        let mut app = make_app();
        let request = make_tx();
        app.execute(request).await.unwrap();

        let handle = app.take_checkpoint().await.unwrap().into_owned();

        let full_cp = app.resolve_checkpoint(handle).await.unwrap();

        assert_eq!(full_cp.base, None);
        assert_eq!(full_cp.blocks.len(), 1);

        assert!(verify_chain::<Sha256, _>(genesis_hash::<Sha256>().as_slice(), &full_cp.blocks));
    }

    #[tokio::test]
    async fn checkpoint_zero() {
        let _ = tracing_subscriber::fmt::try_init();

        let mut app = make_app();

        let handle = app.take_checkpoint().await.unwrap().into_owned();

        let full_cp = app.resolve_checkpoint(handle).await.unwrap();

        let block = full_cp;
        assert_eq!(block.base, None);
        assert_eq!(block.blocks.len(), 0);
    }

    #[tokio::test]
    async fn checkpoint_two() {
        let _ = tracing_subscriber::fmt::try_init();

        let mut app = make_app();
        let request = make_tx();
        app.execute(request).await.unwrap();
        let request = make_tx();
        app.execute(request).await.unwrap();

        let handle = app.take_checkpoint().await.unwrap().into_owned();

        let block = app.resolve_checkpoint(handle).await.unwrap();

        assert_eq!(block.base, None);
        assert_eq!(block.blocks.len(), 2);

        assert!(verify_chain::<Sha256, _>(genesis_hash::<Sha256>().as_slice(), &block.blocks));
    }

    #[tokio::test]
    async fn read_all() {
        let _ = tracing_subscriber::fmt::try_init();

        let mut app = make_app();
        // 5 blocks
        for _i in 0..5usize {
            let request = make_tx();
            app.execute(request).await.unwrap();
        }

        let read = Command::read(0);
        let req = Request::builder()
            .payload(to_vec(&read).unwrap().into())
            .flag(RequestFlags::READ_ONLY)
            .build();
        let mut response = app.execute(Message::new(100, 0, req)).await.unwrap();
        let response: Blocks = Blocks::decode(&mut response.inner.payload).unwrap();

        assert_eq!(response.block.len(), 5);
        assert_eq!(response.base, None);

        for block in &response.block {
            tracing::info!("{}", block.header.as_ref().unwrap())
        }

        let gen_hash = genesis_hash::<Sha256>();
        verify_chain::<Sha256, _>(&gen_hash, &response.block);
    }

    #[tokio::test]
    async fn read_subset() {
        let _ = tracing_subscriber::fmt::try_init();

        let mut app = make_app();
        // 5 blocks
        for _i in 0..5usize {
            let request = make_tx();
            app.execute(request).await.unwrap();
        }

        let read = Command::read(3);
        let req = Request::builder()
            .payload(to_vec(&read).unwrap().into())
            .flag(RequestFlags::READ_ONLY)
            .build();
        let mut response = app.execute(Message::new(100, 0, req)).await.unwrap();
        let response = Blocks::decode(&mut response.inner.payload).unwrap();

        for block in &response.block {
            tracing::info!("{}", block.header.as_ref().unwrap())
        }

        assert_eq!(response.block.len(), 3);
        assert_eq!(response.base, None);
    }

    #[tokio::test]
    async fn delete_blocks() {
        let _ = tracing_subscriber::fmt::try_init();

        let mut app = make_app();
        // 5 blocks
        for _i in 0..5usize {
            let request = make_tx();
            app.execute(request).await.unwrap();
        }

        let read = Command::read(0);
        let req = Request::builder()
            .payload(to_vec(&read).unwrap().into())
            .flag(RequestFlags::READ_ONLY)
            .build();
        let mut response = app.execute(Message::new(100, 0, req)).await.unwrap();
        let response = Blocks::decode(&mut response.inner.payload).unwrap();

        let last_header = response.block[2].header.clone().unwrap();
        let signature = app.keys.sign_header(&last_header);
        let delete_cmd = Command::delete(last_header.clone(), signature.clone());
        let mut delete = make_message(delete_cmd);
        delete.inner.flags.set(RequestFlags::READ_ONLY, true);
        let _response = app.execute(delete.clone()).await.unwrap();

        // read should be shorter
        let read = Command::read(0);
        let req = Request::builder()
            .payload(to_vec(&read).unwrap().into())
            .flag(RequestFlags::READ_ONLY)
            .build();
        let mut response = app.execute(Message::new(100, 0, req)).await.unwrap();
        let response = Blocks::decode(&mut response.inner.payload).unwrap();

        for block in &response.block {
            tracing::info!("{}", block.header.as_ref().unwrap())
        }

        // blocks 4, 5
        assert_eq!(response.block.len(), 2);
        assert_eq!(
            response.base,
            Some(Delete {
                header: Some(last_header),
                signature,
            })
        );
    }

    #[tokio::test]
    async fn checkpoint_then_delete() {
        let _ = tracing_subscriber::fmt::try_init();

        let mut app = make_app();
        // 5 blocks
        for _i in 0..5usize {
            let request = make_tx();
            app.execute(request).await.unwrap();
        }

        let handle = app.take_checkpoint().await.unwrap().into_owned();

        let read = Command::read(0);
        let req = Request::builder()
            .payload(to_vec(&read).unwrap().into())
            .flag(RequestFlags::READ_ONLY)
            .build();
        let mut response = app.execute(Message::new(100, 0, req)).await.unwrap();
        let response = Blocks::decode(&mut response.inner.payload).unwrap();

        let last_header = response.block[2].header.clone().unwrap();
        let signature = app.keys.sign_header(&last_header);
        let delete_cmd = Command::delete(last_header.clone(), signature.clone());
        let mut delete = make_message(delete_cmd);
        delete.inner.flags.set(RequestFlags::READ_ONLY, true);
        let _response = app.execute(delete.clone()).await.unwrap();

        let full_cp = app.resolve_checkpoint(handle).await.unwrap();

        assert_eq!(
            full_cp.base,
            Some(Delete {
                header: Some(last_header),
                signature,
            })
        );
        assert_eq!(full_cp.blocks.len(), 2);
    }

    #[test]
    fn hash_state() {
        let header = BlockHeader {
            number: 5,
            previous_hash: vec![5, 5, 5, 5, 5, 5],
            data_hash: vec![8, 8, 8, 8, 8, 8],
        };

        let state = RailchainState {
            cutter: CutterState {
                buffer: Vec::default().into(),
            },
            chain: BlockchainState {
                head: Cow::Borrowed(&header),
            },
        };

        let bytes = rmp_serde::to_vec(&state).unwrap();
        println!("{:?}", bytes);
    }
}
