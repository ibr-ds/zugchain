use std::{borrow::Cow, mem::size_of, path::PathBuf};

use argh::FromArgs;
use bytes::Bytes;
use rc_application::app::{
    chain::{Blockchain, BlockchainState, CutterState},
    RailchainState,
};
use rc_blockchain::{
    chainproto::BlockHeader,
    export::{transaction::TxType, Transaction},
    genesis::genesis_block,
    storage::{Files, StorageMode},
};
use ring::digest::{digest, SHA256};
use themis_core::{
    authentication::{ed25519::Ed25519Auth, AuthenticatorExt},
    config,
    config::Peer,
    net::Message,
    protocol::{Checkpoint, Framework},
};

/// generate blocks
#[derive(Debug, FromArgs)]
struct Args {
    /// transaction size
    #[argh(option, default = "1024")]
    txsize: usize,
    /// tx per block
    #[argh(option, default = "10")]
    blocksize: usize,
    /// out dir
    #[argh(option)]
    out: PathBuf,
    /// number of blocks
    #[argh(option)]
    blocks: usize,
    /// themis config
    #[argh(option)]
    config: String,
}

fn generate_proof(header: &BlockHeader, interval: u64, peers: &[(Peer, PathBuf)]) -> (u64, Vec<Message<Checkpoint>>) {
    let sequence = header.number * interval;

    let checkpoint = RailchainState::new(
        CutterState {
            buffer: Vec::default().into(),
        },
        BlockchainState {
            head: Cow::Borrowed(header),
        },
    );
    let bytes = rmp_serde::to_vec(&checkpoint).unwrap();
    let digest = digest(&SHA256, bytes.as_ref());

    let checkpoint = Checkpoint::new(sequence, Bytes::copy_from_slice(digest.as_ref()));

    let mut proofs = Vec::new();
    for (peer, private_key) in peers {
        let mut signer = Ed25519Auth::new(private_key).unwrap();
        let mut message = Message::broadcast(peer.id as u64, checkpoint.clone());

        AuthenticatorExt::<Framework>::sign_unpacked(&mut signer, &mut message).unwrap();

        proofs.push(message);
    }

    (sequence, proofs)
}

fn main() {
    let args: Args = argh::from_env();

    let config = config::load_from_paths(&[&args.config]).expect("config");
    let peers: Vec<Peer> = config.get("peers").unwrap();
    let keyfiles: Vec<PathBuf> = peers
        .iter()
        .map(|p| config.get(&format!("peers[{}].private_key", p.id)).unwrap())
        .collect();

    let peers: Vec<_> = peers.into_iter().zip(keyfiles).collect();

    let storage = Files::new(
        args.out,
        StorageMode::Bootstrap {
            block: genesis_block(),
            clean: true,
        },
    );
    let mut blockchain = Blockchain::new(storage);

    for i in 1..=args.blocks {
        let mut envs = Vec::with_capacity(args.blocksize);
        for t in 0..args.blocksize {
            let mut payload = vec![1; args.txsize];
            let tx_idx = (i * t) as u64;
            payload[..size_of::<u64>()].copy_from_slice(&tx_idx.to_le_bytes());
            let env = Transaction {
                payload: payload.into(),
                tx_type: TxType::Data.into(),
            };
            envs.push(env);
        }
        blockchain.create_block(envs);

        if i == args.blocks {
            let cp = config.get("pbft.checkpoint_interval").unwrap();
            let (sequence, proof) = generate_proof(blockchain.head(), cp, &peers);
            blockchain.store_proof(sequence, proof)
        }
    }
}
