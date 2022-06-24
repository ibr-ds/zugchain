use std::{
    convert::TryInto,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures_util::future::try_join_all;
use prost::Message as _;
use rc_blockchain::{
    chainproto::{Block, BlockHeader},
    display::DisplayBytes,
    export::{CheckpointProof, CheckpointedBlocks, Command},
    genesis::GENESIS_HEAD,
    hashing::{hash_header, verify_chain},
    to_vec,
};
use ring::{digest::SHA256, hmac::HMAC_SHA256};
use ring_compat::digest::Sha256;
use serde::Serialize;
use themis_core::{
    app::{Client, Request, RequestFlags, Response, ResponseFlags},
    authentication::{ed25519::Ed25519Auth, AuthenticatorExt},
    comms::{Receiver, Sender},
    config::Peer,
    net::{Endpoint, Message, RawMessage},
    protocol::{Checkpoint, Framework},
};
use tokio::{sync::Semaphore, time::sleep};
use tokio_stream::StreamExt;

use crate::ClientConfig;

pub struct Exporter2 {
    outgoing_tx: Sender<RawMessage<Client>>,
    incoming_rx: Receiver<RawMessage<Client>>,
    me: u64,
    peers: Vec<Peer>,
    signer: Ed25519Auth,
}

const F: usize = 1;

impl Exporter2 {
    pub async fn connect(client: ClientConfig, peers: Vec<Peer>) -> anyhow::Result<Self> {
        // let handshaker =
        //     themis_core::authentication::ed25519::Ed25519Factory::new(client.id, client.keyfile, peers.to_owned());

        let handshaker = themis_core::authentication::hmac::HmacFactory::new(client.id, HMAC_SHA256);

        let (outgoing_tx, outgoing_rx) = themis_core::comms::bounded(16);
        let (incoming_tx, incoming_rx) = themis_core::comms::bounded(16);
        let group = themis_core::net::Group::new(outgoing_rx, incoming_tx, handshaker.clone(), None);

        let conn_timeout = sleep(Duration::from_secs(60));
        if let Some(listen) = client.listen {
            let server = Endpoint::bind(client.id, listen, group.conn_sender(), handshaker.clone()).await;
            tokio::spawn(server.listen());
        } else {
            let sem = Arc::new(Semaphore::new(0));
            let connections = peers.iter().map(|peer| {
                let sem = sem.clone();
                let sink = group.conn_sender();
                let handshake = handshaker.clone();
                async move {
                    themis_core::net::connect((peer.host.as_str(), peer.client_port), sink, handshake).await?;
                    let permits = if peer.id == 0 { 10 } else { 1 };
                    sem.add_permits(permits);
                    Ok::<_, themis_core::Error>(())
                }
            });
            tokio::select! {
                biased;
                r = sem.acquire_many(12) => { let _perm = r.unwrap(); }
                r = try_join_all(connections) => { r.unwrap(); }
                _ = conn_timeout => { anyhow::bail!("connection timed out") }
            };
        }
        let _ = tokio::spawn(group.execute());
        sleep(Duration::from_millis(500)).await;

        let signer =
            themis_core::authentication::ed25519::Ed25519Factory::new(client.id, client.keyfile, peers.clone());
        let signer = signer.create_signer();

        Ok(Self {
            outgoing_tx,
            incoming_rx,
            me: client.id,
            peers,
            signer,
        })
    }

    pub async fn delete_blocks(&mut self, base: BlockHeader) -> anyhow::Result<()> {
        let base_hash = hash_header::<Sha256>(&base);
        let signature = self.signer.sign_bytes(&base_hash);
        let delete = Command::delete(base, Vec::from(signature.as_ref()));

        let delete = to_vec(&delete)?;
        let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis().try_into()?;
        let mut delete_msg = Message::broadcast(self.me, Request::new(time, delete.into()));
        delete_msg.inner.flags.set(RequestFlags::READ_ONLY, true);

        self.outgoing_tx.send(delete_msg.pack()?).await?;

        let mut responses = (&mut self.incoming_rx).take(F + 1);

        while (responses.next().await).is_some() {}

        Ok(())
    }

    pub async fn fetch_blocks(&mut self, earliest: u64) -> anyhow::Result<FetchResponse> {
        println!("fechting blocks");
        let not_hashed = 0;
        let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis().try_into()?;
        let request = Command::read2(earliest);
        let request = to_vec(&request)?;
        let message = Message::broadcast(self.me, Request::new(time, request.into()));

        for peer in &self.peers {
            let mut message = message.clone();
            message.inner.flags.set(RequestFlags::READ_ONLY, true);
            if peer.id != not_hashed {
                message.inner.flags.set(RequestFlags::HASH_REPLY, true);
            }
            message.destination = peer.id.try_into()?;
            self.outgoing_tx.send(message.pack()?).await?;
        }

        let mut response = Self::fetch_step1(&mut self.incoming_rx, not_hashed as u64).await;
        let verify_span = tracing::info_span!(target: "__measure::export::verify", "verify");

        let (source, proof) = verify_span.in_scope(|| self.verify1(&mut response));
        let head = proof.head.as_ref().unwrap().number;

        let base = verify_span.in_scope(|| Self::verify_base(&mut self.signer, &response).clone());
        let highest_block = response.blocks.last().map(|b| b.number()).unwrap_or(base.number);

        tracing::info!(head, highest_block, base = base.number, "step1");
        if head > highest_block && head > base.number {
            let blocks = self
                .fetch_step2(source, base.number.max(highest_block), head)
                .await
                .unwrap();
            response.blocks.extend(blocks);
        }

        let verified = verify_span.in_scope(|| {
            let base_hash = hash_header::<Sha256>(&base);
            verify_chain::<Sha256, _>(
                &base_hash,
                response.blocks.iter().skip_while(|b| b.number() <= base.number),
            )
        });

        let highest_block = response.blocks.last().map(|b| b.number()).unwrap_or(base.number);
        assert_eq!(head, highest_block);
        assert!(verified);

        tracing::info!(
            highest_block,
            base = base.number,
            num = response.blocks.len(),
            "fetch complete"
        );
        Ok(FetchResponse {
            blocks: response.blocks,
            proof,
        })
    }

    async fn fetch_step1(response_recv: &mut Receiver<RawMessage<Client>>, all_from: u64) -> ResponseSet {
        let mut responses = ResponseSet {
            blocks: Vec::default(),
            proof: Vec::default(),
        };

        let mut received = 0;
        let mut has_full = false;

        while let Some(response) = response_recv.next().await {
            if response.source == all_from {
                has_full = true;
            }
            println!("response from {}", response.source);
            let response: Message<Response> = response.unpack().expect("not response");
            tracing::debug!(source = response.source, "response");
            if response.inner.flags.contains(ResponseFlags::HASHED) {
                let payload = CheckpointProof::decode(response.inner.payload).expect("not proof");
                responses.proof.push((response.source, payload));
            } else {
                let payload = CheckpointedBlocks::decode(response.inner.payload).expect("not blocks");
                responses.proof.push((response.source, payload.proof.unwrap()));
                let blocks = payload
                    .blocks
                    .into_iter()
                    .filter_map(|b| Block::decode(b).ok())
                    .collect();
                responses.blocks = blocks;
            }

            received += 1;
            if received > 2 * F && has_full {
                break;
            }
        }

        responses
    }

    fn verify_proof(signer: &mut Ed25519Auth, head: &BlockHeader, source: u64, proof: &[Message<Checkpoint>]) -> bool {
        let state = RailchainState {
            cutter: Holder::default(),
            chain: Header { header: head },
        };
        let state = rmp_serde::to_vec(&state).unwrap();
        let digest = ring::digest::digest(&SHA256, &state);

        let p = DisplayBytes(&digest);
        tracing::trace!(digest=%p, "verify");

        proof.iter().all(|proof| {
            tracing::trace!(source = proof.source, "verify proof");
            if proof.inner.digest != digest.as_ref() {
                tracing::error!("digest mismatch");
                return false;
            }

            if proof.source == source {
                return true;
            } else if let Err(e) = AuthenticatorExt::<Framework>::verify_unpacked(signer, proof) {
                tracing::error!(%e, "bad signature");
                return false;
            }
            true
        })
    }

    fn unpack_checkpoints(set: &CheckpointProof) -> Vec<Message<Checkpoint>> {
        let quorum = set.quorum.as_ref().unwrap();
        quorum
            .messages
            .iter()
            .map(|m| {
                let m: RawMessage<Framework> = rmp_serde::from_slice(&m).unwrap();
                let m: Message<Checkpoint> = m.unpack().unwrap();
                m
            })
            .collect()
    }

    fn verify1(&mut self, set: &mut ResponseSet) -> (u64, CheckpointProof) {
        set.proof
            .sort_unstable_by_key(|proof| proof.1.head.as_ref().unwrap().number);
        if let Some(proof) = set
            .proof
            .iter()
            .map(|(source, proof)| {
                let messages = Self::unpack_checkpoints(proof);
                (source, proof, messages)
            })
            .find(|(source, proof, messages)| {
                Self::verify_proof(&mut self.signer, proof.head.as_ref().unwrap(), **source, messages)
            })
        {
            (*proof.0, proof.1.clone())
        } else {
            panic!("not valid response");
        }
    }

    fn verify_base<'a>(signer: &mut Ed25519Auth, set: &'a ResponseSet) -> &'a BlockHeader {
        static GENESIS: BlockHeader = GENESIS_HEAD;

        set.proof
            .iter()
            .filter_map(|(_, proof)| proof.base.as_ref())
            .filter(|delete| {
                let digest = hash_header::<Sha256>(delete.header.as_ref().unwrap());
                signer.verify_own(&digest, &delete.signature).is_ok()
            })
            .map(|delete| delete.header())
            .max_by_key(|b| b.number)
            .unwrap_or(&GENESIS)
    }

    async fn fetch_step2(&mut self, from: u64, base: u64, head: u64) -> anyhow::Result<Vec<Block>> {
        let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis().try_into()?;
        let request = Command::read0(base, head);
        let request = to_vec(&request)?;
        let mut message = Message::new(self.me, from, Request::new(time, request.into()));
        message.inner.flags.set(RequestFlags::READ_ONLY, true);

        self.outgoing_tx.send(message.pack()?).await.unwrap();
        let response = (&mut self.incoming_rx)
            .filter(|m| m.source == from)
            .next()
            .await
            .unwrap();
        let response: Message<Response> = response.unpack().unwrap();

        let blocks = CheckpointedBlocks::decode(response.inner.payload)?;
        let blocks = blocks
            .blocks
            .into_iter()
            .filter_map(|b| Block::decode(b).ok())
            .collect();
        Ok(blocks)
    }
}

pub struct ResponseSet {
    pub blocks: Vec<Block>,
    pub proof: Vec<(u64, CheckpointProof)>,
}

pub struct FetchResponse {
    pub blocks: Vec<Block>,
    pub proof: CheckpointProof,
}

// checkpoint format internals

#[derive(Debug, Serialize)]
pub struct RailchainState<'a> {
    cutter: Holder,
    chain: Header<'a>,
}

#[derive(Debug, Serialize, Default)]
struct Holder {
    cutter: Vec<u64>,
}

#[derive(Debug, Serialize)]
pub struct Header<'a> {
    header: &'a BlockHeader,
}

pub fn pack_checkpoints(set: &[Message<Checkpoint>]) -> Vec<Vec<u8>> {
    set.iter()
        .map(|m| {
            let m: RawMessage<Framework> = m.pack().unwrap();

            rmp_serde::to_vec(&m).unwrap()
        })
        .collect()
}

#[cfg(test)]
mod test {
    use rc_blockchain::chainproto::BlockHeader;

    use super::{Header, Holder, RailchainState};
    #[test]
    fn hash_state() {
        let header = BlockHeader {
            number: 5,
            previous_hash: vec![5, 5, 5, 5, 5, 5],
            data_hash: vec![8, 8, 8, 8, 8, 8],
        };

        let state = RailchainState {
            cutter: Holder { cutter: Vec::default() },
            chain: Header { header: &header },
        };

        let bytes = rmp_serde::to_vec(&state).unwrap();
        println!("{:?}", bytes);
    }
}
