use std::collections::HashMap;

use bytes::Bytes;
use themis_core::protocol::{Proposal, Slots};

pub struct AssignedRequests {
    by_payload: HashMap<Bytes, u64>,
    by_sequence: Slots<Proposal<Bytes>>,
    backlog_capacity: usize,
}

impl AssignedRequests {
    pub fn new(high_mark: usize, backlog: usize) -> Self {
        Self {
            by_payload: HashMap::default(),
            by_sequence: Slots::with_first(1, high_mark + backlog),
            backlog_capacity: backlog,
        }
    }

    pub fn assign(&mut self, proposal: &Proposal, sequence: u64) {
        for request in proposal {
            self.by_payload.insert(request.inner.payload.clone(), sequence);
        }
        match proposal {
            Proposal::Batch(batch) => self.by_sequence.insert(
                sequence,
                Proposal::Batch(batch.iter().map(|r| r.inner.payload.clone()).collect()),
            ),
            Proposal::Single(single) => self
                .by_sequence
                .insert(sequence, Proposal::Single(single.inner.payload.clone())),
        };
    }

    pub fn checkpoint(&mut self, low_mark: u64) {
        let new_low = low_mark.saturating_sub(self.backlog_capacity as u64);
        let advance = new_low + 1 - self.by_sequence.first_idx();

        tracing::trace!(to = new_low, delta = advance, "advancing");

        let removals = self.by_sequence.advance_iter(advance);

        for payload in removals.flatten() {
            self.by_payload.remove(&payload);
        }

        assert_eq!(self.by_sequence.first_idx(), new_low + 1);
    }

    pub fn is_assigned(&self, payload: &[u8]) -> bool {
        self.by_payload.contains_key(payload)
    }

    pub fn len(&self) -> usize {
        self.by_payload.len()
    }

    #[allow(unused)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
