use std::collections::HashMap;

use bytes::Bytes;
use slotmap::SlotMap;
use themis_core::{app::Request, net::Message};

use tokio_util::time::delay_queue::Key;

pub struct Entry {
    pub digest: Bytes,
    pub proposal: Message<Request>,
    pub timeout: Key,
}

slotmap::new_key_type! {
    struct RKey;
}

pub struct RequestStore {
    requests: SlotMap<RKey, Entry>,
    by_payload: HashMap<Bytes, RKey>,
    by_digest: HashMap<Bytes, RKey>,
}

impl RequestStore {
    pub fn new() -> Self {
        Self {
            requests: SlotMap::default(),
            by_payload: HashMap::default(),
            by_digest: HashMap::default(),
        }
    }

    // pub fn insert(&mut self, message: Message<Request>, timeout: Key) {
    //     let proposal = Proposal::Single(message);
    //     let digest = hash_proposal(&proposal);
    //     self.insert_with_digest(proposal.assert_single(), digest, timeout);
    // }

    pub fn insert_with_digest(&mut self, message: Message<Request>, digest: Bytes, timeout: Key) {
        let payload = message.inner.payload.clone();
        let key = self.requests.insert(Entry {
            proposal: message,
            digest: digest.clone(),
            timeout,
        });
        self.by_payload.insert(payload, key);
        self.by_digest.insert(digest, key);
    }

    pub fn remove_by_payload(&mut self, bytes: &[u8]) -> Option<Entry> {
        if let Some(entry) = self.by_payload.remove(bytes).and_then(|key| self.requests.remove(key)) {
            self.by_digest.remove(&entry.digest);
            Some(entry)
        } else {
            None
        }
    }

    pub fn remove_by_digest(&mut self, digest: &[u8]) -> Option<Entry> {
        if let Some(entry) = self.by_digest.remove(digest).and_then(|key| self.requests.remove(key)) {
            self.by_payload.remove(&entry.proposal.inner.payload);
            Some(entry)
        } else {
            None
        }
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Entry> {
        self.requests.values_mut()
    }

    #[allow(unused)]
    pub fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }

    pub fn drain(&mut self) -> impl Iterator<Item = Message<Request>> + '_ {
        // I have no idea why this lint triggers _here_ of all places
        // But bytes::Bytes is a common false positive
        #[allow(clippy::mutable_key_type)]
        let by_digest = &mut self.by_digest;
        #[allow(clippy::mutable_key_type)]
        let by_payload = &mut self.by_payload;
        self.requests.drain().map(move |(_key, e)| {
            by_digest.remove(&e.digest);
            by_payload.remove(&e.proposal.inner.payload);
            e.proposal
        })
    }
}
