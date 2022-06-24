use std::{
    fs,
    path::{Path, PathBuf},
};

use bytes::Bytes;
use prost::Message;

use crate::{
    chainproto,
    export::Delete,
    proto::{
        export::CurrentCheckpoint,
        storage::{Base, Head},
    },
    storage::Storage,
};

#[derive(Debug)]
pub enum StorageMode {
    Bootstrap { block: chainproto::Block, clean: bool },
    Resume { header: chainproto::BlockHeader },
    Read,
}

const BASEFILE: &str = "base";
const HEADFILE: &str = "head";
const CP_FILE: &str = "checkpoint";

pub struct Files {
    directory: PathBuf,
    base: Delete,
    head: chainproto::BlockHeader,
    checkpoint: CurrentCheckpoint,
}

impl Files {
    pub fn new(directory: PathBuf, bootstrap: StorageMode) -> Self {
        tracing::info!("Files::new: {}, {:?}", directory.display(), bootstrap);
        match bootstrap {
            StorageMode::Bootstrap { block, clean } => {
                if clean {
                    if let Err(e) = fs::remove_dir_all(&directory) {
                        tracing::error!("remove_dir: {}", e);
                    }
                }
                fs::create_dir(&directory).unwrap();

                let base = Base {
                    header: block.header.clone(),
                    signature: Vec::default(),
                };

                let mut files = Self {
                    directory,
                    base,
                    head: block.header().clone(),
                    checkpoint: CurrentCheckpoint::default(),
                };

                files.write_base();
                files.store_block(block);

                files
            }
            StorageMode::Resume { header } => {
                fs::remove_dir_all(&directory).unwrap();
                fs::create_dir(&directory).unwrap();

                let base = Base {
                    header: Some(header.clone()),
                    signature: Vec::default(),
                };

                let files = Self {
                    directory,
                    base,
                    head: header,
                    checkpoint: CurrentCheckpoint::default(),
                };
                files.write_base();
                files.write_head();

                files
            }
            StorageMode::Read => {
                assert!(directory.is_dir(), "block directory does not exist");

                let head = Files::read_head(&directory);
                let base = Files::read_base(&directory);

                let cpfile = directory.join(CP_FILE);
                let checkpoint = fs::read(&cpfile)
                    .map(|checkpoint| CurrentCheckpoint::decode(checkpoint.as_slice()).unwrap())
                    .unwrap_or_default();

                tracing::info!(?base, ?head, checkpoint = checkpoint.sequence, "read");

                let mut files = Self {
                    directory,
                    base,
                    head: chainproto::BlockHeader::default(),
                    checkpoint,
                };

                let headblock = files
                    .read_header(head.number)
                    .or_else(|| files.read_header(head.number - 1))
                    .unwrap();

                files.head = headblock;

                files
            }
        }
    }

    fn filename(&self, sequence: u64) -> PathBuf {
        let mut path = self.directory.join(format!("{}", sequence));
        path.set_extension("block");
        path
    }

    fn read_base(directory: &Path) -> Base {
        let file = directory.join(BASEFILE);
        let bytes = fs::read(&file).expect("read base");
        Base::decode(bytes.as_slice()).unwrap()
    }

    fn read_head(directory: &Path) -> Head {
        let file = directory.join(HEADFILE);
        let bytes = fs::read(&file).expect("read head");
        Head::decode(bytes.as_slice()).unwrap()
    }

    fn write_base(&self) {
        let file = self.directory.join(BASEFILE);
        let bytes = encode(&self.base);
        fs::write(file, bytes).expect("write base");
    }

    fn write_head(&self) {
        let file = self.directory.join(HEADFILE);

        let head = Head {
            number: self.head.number,
        };
        let bytes = encode(&head);
        fs::write(file, bytes).expect("write head");
    }

    fn store_proof(&mut self, proof: CurrentCheckpoint) {
        self.checkpoint = proof;
        let file = self.directory.join(CP_FILE);
        fs::write(file, encode(&self.checkpoint)).expect("write checkpont")
    }
}

fn encode<M: prost::Message>(message: &M) -> Vec<u8> {
    let mut write_buf = Vec::with_capacity(message.encoded_len());
    message.encode(&mut write_buf).unwrap();
    write_buf
}

impl Storage for Files {
    fn store_proof(&mut self, proof: CurrentCheckpoint) {
        self.store_proof(proof)
    }

    fn get_proof(&self) -> &CurrentCheckpoint {
        &self.checkpoint
    }

    fn store_block(&mut self, block: chainproto::Block) {
        let sequence = block.header.as_ref().unwrap().number;

        let filename = self.filename(sequence);

        let mut write_buf = Vec::with_capacity(block.encoded_len());
        block.encode(&mut write_buf).unwrap();
        fs::write(&filename, &write_buf).unwrap();

        self.head = block.header().clone();
        self.write_head();
    }

    fn prune_chain(&mut self, delete: Delete) {
        for index in self.base.number()..=delete.number() {
            let filename = self.filename(index);

            if filename.is_file() {
                let _ = fs::remove_file(filename);
            }
        }

        self.base = delete;
        self.write_base()
    }

    fn read_block(&self, sequence: u64) -> Option<chainproto::Block> {
        let read_buf: Bytes = self.read_block_binary(sequence)?.into();
        let block = chainproto::Block::decode(read_buf)
            .map_err(|e| {
                tracing::error!(%e, "read_block");
                e
            })
            .ok()?;

        Some(block)
    }

    fn read_block_binary(&self, sequence: u64) -> Option<Vec<u8>> {
        let filename = self.filename(sequence);

        if !filename.is_file() {
            return None;
        }

        let read_buf = fs::read(&filename).ok()?;
        Some(read_buf)
    }

    // fn delete_block(&mut self, index: u64) {
    //     let mut filename = self.directory.clone();
    //     filename.push(Files::filename(index));

    //     if filename.is_file() {
    //         let _ = fs::remove_file(filename);
    //     }
    // }

    fn read_header(&self, index: u64) -> Option<chainproto::BlockHeader> {
        // FIXME: Don't readly whole block, read more efficiently
        let block = self.read_block(index)?;
        tracing::info!(?block.header, "read_header");
        block.header
    }

    fn base(&self) -> &Delete {
        &self.base
    }

    fn head(&self) -> &chainproto::BlockHeader {
        &self.head
    }
}
