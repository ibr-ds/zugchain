#![feature(type_alias_impl_trait)]

use std::{convert::TryInto, future::Future, task::Poll, time::Duration};

use app::RequestFlags;
use assigned_reqs::AssignedRequests;
use bytes::Bytes;
use futures_util::{
    stream::{FusedStream, Stream},
    StreamExt,
};
use request_queue::RequestStore;
use ring::digest;
use themis_core::{
    app::{self, Client, Command, Response},
    authentication::Authenticator,
    comms::Sender,
    config::Config,
    net::{DisplayBytes, Message, RawMessage, Sequenced},
    protocol::{Event, Proposal, Protocol, Protocol2, ProtocolTag, Result},
};

use themis_pbft::{
    messages::{Assign, Forward, PBFTTag, PrePrepare},
    ProposalHasher, PBFT,
};
use tokio_util::time::DelayQueue;

mod assigned_reqs;
mod request_queue;
#[cfg(test)]
mod test;

type PBFTMessage = RawMessage<themis_pbft::messages::PBFT>;
const SOFT_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Debug, serde::Deserialize)]
struct RcConfig {
    backlog: usize,
}

impl Default for RcConfig {
    fn default() -> Self {
        Self { backlog: 1 }
    }
}

pub struct RcBft2 {
    request_queue: RequestStore,
    soft_timeouts: DelayQueue<Bytes>,
    pbft: PBFT<RcBft2Hasher>,
    assigned: AssignedRequests,
    replicas: Sender<PBFTMessage>,
    _config: RcConfig,
}

impl RcBft2 {
    pub fn new(
        id: u64,
        replicas: Sender<PBFTMessage>,
        clients: Sender<RawMessage<Client>>,
        app: Sender<Command>,
        verifier: Box<dyn Authenticator<ProtocolTag<PBFTTag>> + Send + Sync + 'static>,
        themis_config: &Config,
    ) -> Self {
        let config: RcConfig = themis_config.get("railchain.bft").unwrap_or_default();

        let pbft = PBFT::new_with_hasher(
            id,
            replicas.clone(),
            clients,
            app,
            verifier,
            themis_config,
            RcBft2Hasher,
        );
        Self {
            soft_timeouts: DelayQueue::default(),
            request_queue: RequestStore::new(),
            assigned: AssignedRequests::new(pbft.config().high_mark_delta, config.backlog),
            pbft,
            replicas,
            _config: config,
        }
    }
}

impl RcBft2 {
    async fn on_pre_prepare(&mut self, message: PBFTMessage) -> Result<()> {
        let pre_prepare: Message<PrePrepare> = message.unpack()?;
        self.pbft.handle_pre_prepare(pre_prepare).await?;

        // if let Some(waiting) = self.request_queue.remove_by_digest(&pre_prepare.inner.request) {
        //     tracing::info!("Found our request in pre-prepare");
        //     self.pbft.handle_new_proposal(Proposal::Single(waiting.proposal)).await?;
        //     self.soft_timeouts.remove(&waiting.timeout);
        // }

        Ok(())
    }

    async fn on_assign(&mut self, message: PBFTMessage) -> Result<()> {
        let assign: Message<Assign> = message.unpack()?;
        self.pbft.handle_assign(assign.clone()).await?;

        for proposal in &assign.inner.batch {
            if let Some(waiting) = self.request_queue.remove_by_payload(&proposal.inner.payload) {
                tracing::info!("Found equivalent request in proposal");
                self.soft_timeouts.remove(&waiting.timeout);
            }
        }
        self.assigned.assign(&assign.inner.batch, assign.inner.sequence);

        Ok(())
    }

    async fn on_new_view(&mut self, message: PBFTMessage) -> Result<()> {
        self.pbft.on_message(message).await?;

        //TODO fix assignments after view change
        self.soft_timeouts.clear();
        for entry in self.request_queue.iter_mut() {
            let key = self.soft_timeouts.insert(entry.digest.clone(), SOFT_TIMEOUT);
            entry.timeout = key;
        }
        Ok(())
    }

    async fn on_soft_timeout(&mut self, request: Bytes) -> Result<()> {
        if let Some(waiting) = self.request_queue.remove_by_digest(&request) {
            {
                let source = waiting.proposal.source;
                let seq = waiting.proposal.sequence();
                tracing::trace!(
                    target: "__measure::soft_timeout", parent: None,
                    source,
                    seq, "st"
                );
            }
            tracing::debug!(d=%DisplayBytes(&request), p=self.pbft.primary(), "forwarding");
            let proposal = Proposal::Single(waiting.proposal);
            let broadcast = Message::broadcast(self.pbft.id(), Forward(proposal.clone()));
            let broadcast = broadcast.pack()?;
            self.replicas.send(broadcast).await?;
            self.pbft.handle_new_proposal(proposal).await?;
        }

        Ok(())
    }

    fn filter_proposal(&self, proposal: Proposal) -> Option<Proposal> {
        let proposal: Proposal = proposal
            .into_iter()
            .filter(|request| !self.assigned.is_assigned(&request.inner.payload))
            .collect();
        if proposal.is_empty() {
            None
        } else {
            Some(proposal)
        }
    }

    #[tracing::instrument(skip(self, proposal))]
    async fn handle_new_proposal(&mut self, proposal: Proposal) -> Result<()> {
        if let Proposal::Single(request) = &proposal {
            if request.inner.flags.contains(RequestFlags::READ_ONLY) {
                self.pbft
                    .app()
                    .send(app::Command::ReadOnlyRequst(request.clone()))
                    .await?;
                return Ok(());
            }
        }
        tracing::info!("on request");
        if self.pbft.is_primary() {
            if let Some(proposal) = self.filter_proposal(proposal) {
                tracing::warn!("proposal");
                let sequence = self.pbft.next_sequence();
                self.pbft.handle_new_proposal(proposal.clone()).await?;
                self.assigned.assign(&proposal, sequence);
            }
        } else {
            //TODO check if assigned
            for request in proposal {
                if self.assigned.is_assigned(&request.inner.payload) {
                    tracing::trace!("assigned, releasing to pbft");
                } else {
                    let proposal = Proposal::Single(request);
                    let digest = RcBft2Hasher::hash_proposal(&proposal);
                    tracing::debug!(digest=%DisplayBytes(&digest), "not assigned, waiting");
                    let key = self.soft_timeouts.insert(digest.clone(), SOFT_TIMEOUT);
                    self.request_queue
                        .insert_with_digest(proposal.assert_single(), digest, key);
                }
            }
        }
        Ok(())
    }

    #[tracing::instrument(skip(self, message))]
    async fn on_forward(&mut self, message: PBFTMessage) -> Result<()> {
        tracing::trace!(target: "__measure::forward", parent: None, "ff");
        let forward: Message<Forward> = message.unpack()?;
        let proposal = forward.inner.0;
        if self.pbft.is_primary() {
            if let Some(proposal) = self.filter_proposal(proposal) {
                let sequence = self.pbft.next_sequence();
                self.pbft.handle_new_proposal(proposal.clone()).await?;
                self.assigned.assign(&proposal, sequence);
            }
        } else {
            let mut to_primary = Vec::new();
            for request in proposal {
                if !self.assigned.is_assigned(&request.inner.payload) {
                    self.pbft.handle_new_proposal(Proposal::Single(request.clone())).await?;
                    to_primary.push(request);
                }
            }
            if !to_primary.is_empty() {
                let proposal: Proposal = to_primary.try_into().unwrap();
                let forward = Message::new(self.pbft.id(), self.pbft.primary(), Forward(proposal)).pack()?;
                self.replicas.send(forward).await?;
            }
        }
        Ok(())
    }

    #[tracing::instrument(skip(self, message))]
    async fn on_view_change(&mut self, message: PBFTMessage) -> Result<()> {
        let view_change = message.unpack()?;
        let old_view = self.pbft.view();
        self.pbft.handle_view_change(view_change).await?;
        // view changed
        if self.pbft.view() > old_view && self.pbft.is_primary() {
            self.soft_timeouts.clear();

            for r in &self.pbft.reorder_buffer {
                tracing::warn!(ts = r.inner.sequence, "reorder from pbft");
            }

            let requests = self.request_queue.drain();

            self.pbft.reorder_buffer.extend(requests);
            tracing::warn!(
                view = self.pbft.view(),
                reorder = self.pbft.reorder_buffer.len(),
                "View Changed"
            );
            for r in &self.pbft.reorder_buffer {
                tracing::warn!(ts = r.inner.sequence, "reorder");
            }
        }
        Ok(())
    }

    async fn message(&mut self, message: RawMessage<themis_pbft::messages::PBFT>) -> Result<()> {
        match message.tag() {
            ProtocolTag::Protocol(PBFTTag::PRE_PREPARE) => self.on_pre_prepare(message).await,
            ProtocolTag::Protocol(PBFTTag::FORWARD) => self.on_forward(message).await,
            ProtocolTag::Protocol(PBFTTag::NEW_VIEW) => self.on_new_view(message).await,
            ProtocolTag::Protocol(PBFTTag::ASSIGN) => self.on_assign(message).await,
            ProtocolTag::Protocol(PBFTTag::VIEW_CHANGE) => self.on_view_change(message).await,
            _ => self.pbft.on_message(message).await,
        }?;
        self.assigned.checkpoint(self.pbft.low_mark());
        Ok(())
    }
}

impl Protocol for RcBft2 {
    type Messages = <PBFT as Protocol>::Messages;
    type Event = FM2Event;
}

impl<'a> Protocol2<'a> for RcBft2 {
    type OnMessage = impl Future<Output = Result<()>> + Send + 'a;

    fn on_message(&'a mut self, message: RawMessage<Self::Messages>) -> Self::OnMessage {
        self.message(message)
    }

    type OnResponse = impl Future<Output = Result<()>> + Send + 'a;

    fn on_response(&'a mut self, message: Message<Response>) -> Self::OnResponse {
        self.pbft.on_response(message)
    }

    type OnRequest = impl Future<Output = Result<()>> + Send + 'a;

    fn on_request(&'a mut self, request: Proposal) -> Self::OnRequest {
        self.handle_new_proposal(request)
    }

    type OnEvent = impl Future<Output = Result<()>> + Send + 'a;

    fn on_event(&'a mut self, event: Self::Event) -> Self::OnEvent {
        async move {
            match event {
                FM2Event::Pbft(e) => self.pbft.on_timeout(e).await?,
                FM2Event::Soft(soft_timeout) => self.on_soft_timeout(soft_timeout).await?,
            }
            Ok(())
        }
    }
}

pub enum FM2Event {
    Pbft(<PBFT as Protocol>::Event),
    Soft(Bytes),
}

impl Stream for RcBft2 {
    type Item = themis_core::protocol::Event<FM2Event>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let soft_timeout = self.soft_timeouts.poll_next_unpin(cx);
        if let Poll::Ready(Some(st)) = soft_timeout {
            let rc = FM2Event::Soft(st.into_inner());
            let soft_timeout = Event::Protocol(rc);
            return Poll::Ready(Some(soft_timeout));
        }

        let pbft_timeout = self.pbft.poll_next_unpin(cx);
        if let Poll::Ready(soft_timeout) = pbft_timeout {
            match soft_timeout {
                Some(Event::Protocol(event)) => return Poll::Ready(Some(Event::Protocol(FM2Event::Pbft(event)))),
                Some(Event::Reorder(t)) => return Poll::Ready(Some(Event::Reorder(t))),
                None => return Poll::Ready(None),
            }
        }

        Poll::Pending
    }
}

impl FusedStream for RcBft2 {
    fn is_terminated(&self) -> bool {
        self.pbft.is_terminated()
    }
}

#[derive(Debug, Default)]
struct RcBft2Hasher;

impl ProposalHasher for RcBft2Hasher {
    fn make_ctx() -> ring::digest::Context {
        ring::digest::Context::new(&ring::digest::SHA256)
    }

    fn hash_request_to(req: &Message<app::Request>, ctx: &mut digest::Context) {
        ctx.update(&req.inner.payload);
    }
}
