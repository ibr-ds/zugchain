use std::time::{SystemTime, UNIX_EPOCH};

use themis_core::{
    app::{Client, Command},
    authentication::dummy::Dummy,
    comms::{unbounded, Receiver},
    config::{self, Config},
    net::RawMessage,
};
use themis_pbft::config::PBFTConfig;

use crate::{PBFTMessage, RcBft2};

struct Session {
    bft: RcBft2,
    r: Receiver<PBFTMessage>,
    a: Receiver<Command>,
    _c: Receiver<RawMessage<Client>>,
}

fn backup() -> Session {
    let replicas = unbounded();
    let clients = unbounded();
    let app = unbounded();

    let verifier = Box::new(Dummy);
    let mut config = config::default();

    add_pbft_config(&mut config);

    let bft = RcBft2::new(1, replicas.0, clients.0, app.0, verifier, &config);
    Session {
        bft,
        r: replicas.1,
        a: app.1,
        _c: clients.1,
    }
}

pub fn add_pbft_config(config: &mut Config) {
    let pbftc = PBFTConfig {
        backup_forwarding: themis_pbft::config::ForwardingMode::None,
        ..Default::default()
    };
    config.set("pbft", pbftc).expect("set pbft");
    config.set("railchain.bft.backlog", 10).expect("set backlog");
}

macro_rules! accept {
    ($e:expr) => {
        assert!(matches!($e.await, Ok(_)));
    };
}

macro_rules! unpack_next {
    ($r:ident) => {
        unwrap_poll!(futures_util::poll!(futures_util::stream::StreamExt::next(&mut $r)))
            .unwrap()
            .unpack()
            .unwrap()
    };
}

macro_rules! assert_next {
    ($r:ident, $m:expr) => {
        assert_eq!($m, unpack_next!($r))
    };
}

macro_rules! unwrap_poll {
    ($e:expr) => {
        match $e {
            std::task::Poll::Pending => panic!("pending"),
            std::task::Poll::Ready(v) => v,
        }
    };
}

macro_rules! assert_pending {
    ($r:ident) => {
        tokio_test::assert_pending!(futures_util::poll!(futures_util::stream::StreamExt::next(&mut $r)))
    };
}

fn timestamp() -> u64 {
    let now = SystemTime::now();
    let epoch = now.duration_since(UNIX_EPOCH).unwrap();
    epoch.as_nanos() as u64
}

mod backup {
    use bytes::Bytes;
    use themis_core::{
        app::{Command, Request},
        net::Message,
        protocol::{Checkpoint, Proposal, Protocol2},
    };
    use themis_pbft::{
        messages::{Assign, Forward, FullCheckpoint, PrePrepare, Prepare},
        ProposalHasher,
    };

    use super::{backup, timestamp};
    use crate::{FM2Event, RcBft2Hasher};

    #[derive(Clone)]
    struct Req {
        m: Proposal,
        d: Bytes,
    }

    fn request_source(source: u64, payload: Bytes) -> Req {
        let request = Message::new(source, 1, Request::new(timestamp(), payload));
        let proposal = Proposal::Single(request);
        let digest = RcBft2Hasher::hash_proposal(&proposal);
        Req { m: proposal, d: digest }
    }

    fn request(payload: Bytes) -> (Req, Req) {
        let request = Message::new(101, 1, Request::new(timestamp(), payload.clone()));
        let proposal = Proposal::Single(request);
        let digest = RcBft2Hasher::hash_proposal(&proposal);

        let mine = Req { m: proposal, d: digest };

        let request = Message::new(100, 1, Request::new(timestamp(), payload));
        let proposal = Proposal::Single(request);
        let digest = RcBft2Hasher::hash_proposal(&proposal);

        let primary = Req { m: proposal, d: digest };

        (mine, primary)
    }

    #[tokio::test]
    async fn local_request_before_preprepare() {
        let super::Session { mut bft, mut r, .. } = backup();

        let (mine, primary) = request(vec![1; 100].into());
        accept!(bft.on_request(mine.m));

        let pre_prepare = Message::broadcast(0, PrePrepare::new(1, 0, primary.d.clone()));
        accept!(bft.on_message(pre_prepare.pack().unwrap()));
        let assign = Message::broadcast(0, Assign::new(1, primary.m));
        accept!(bft.on_message(assign.pack().unwrap()));

        assert_next!(r, Message::broadcast(1, Prepare::new(1, 0, primary.d.clone())));
        assert_pending!(r);

        assert!(bft.request_queue.is_empty());
        assert!(bft.soft_timeouts.is_empty());
        assert_eq!(bft.assigned.len(), 1);
    }

    #[tokio::test]
    async fn preprepare_before_request() {
        // std::env::set_var("RUST_LOG", "trace");
        let _ = tracing_subscriber::fmt::try_init();

        let super::Session { mut bft, mut r, .. } = backup();

        let (mine, primary) = request(vec![1; 100].into());

        let pre_prepare = Message::broadcast(0, PrePrepare::new(1, 0, primary.d.clone()));
        accept!(bft.on_message(pre_prepare.pack().unwrap()));
        let assign = Message::broadcast(0, Assign::new(1, primary.m));
        accept!(bft.on_message(assign.pack().unwrap()));

        assert_next!(r, Message::broadcast(1, Prepare::new(1, 0, primary.d.clone())));
        assert_pending!(r);

        accept!(bft.on_request(mine.m));

        assert!(bft.request_queue.is_empty());
        assert!(bft.soft_timeouts.is_empty());
        assert_eq!(bft.assigned.len(), 1);
    }

    #[tokio::test]
    async fn duplicates_before_and_after() {
        let _ = tracing_subscriber::fmt::try_init();

        let super::Session { mut bft, mut r, .. } = backup();

        let (mine, primary) = request(vec![1; 100].into());
        let two = request_source(102, vec![1; 100].into());
        let three = request_source(103, vec![1; 100].into());

        accept!(bft.on_request(mine.m));
        let pre_prepare = Message::broadcast(0, PrePrepare::new(1, 0, primary.d.clone()));
        accept!(bft.on_message(pre_prepare.pack().unwrap()));
        let assign = Message::broadcast(0, Assign::new(1, primary.m));
        accept!(bft.on_message(assign.pack().unwrap()));

        assert_next!(r, Message::broadcast(1, Prepare::new(1, 0, primary.d.clone())));
        assert_pending!(r);

        let f = Forward(two.m);
        accept!(bft.on_message(Message::broadcast(2, f).pack().unwrap()));
        let f = Forward(three.m);
        accept!(bft.on_message(Message::broadcast(3, f).pack().unwrap()));

        assert!(bft.request_queue.is_empty());
        assert!(bft.soft_timeouts.is_empty());
        assert_eq!(bft.assigned.len(), 1);
    }

    #[tokio::test]
    async fn soft_timeout() {
        let super::Session { mut bft, mut r, .. } = backup();

        let (mine, _primary) = request(vec![1; 100].into());
        accept!(bft.on_request(mine.m.clone()));

        let soft_timeout = FM2Event::Soft(mine.d.clone());
        accept!(bft.on_event(soft_timeout));

        assert_next!(r, Message::broadcast(1, Forward(mine.m)));
        assert_pending!(r);

        assert!(bft.request_queue.is_empty());
        assert_eq!(bft.soft_timeouts.len(), 1);
        assert_eq!(bft.assigned.len(), 0);
        assert!(bft.pbft.is_known(&mine.d));
    }

    #[tokio::test]
    async fn forward_backup() {
        let super::Session { mut bft, mut r, .. } = backup();

        let req = request_source(102, vec![1; 100].into());
        accept!(bft.on_message(Message::broadcast(102, Forward(req.m.clone())).pack().unwrap()));

        assert_next!(r, Message::new(1, 0, Forward(req.m)));
        assert_pending!(r);

        assert!(bft.request_queue.is_empty());
        assert_eq!(bft.soft_timeouts.len(), 0);
        assert_eq!(bft.assigned.len(), 0);
        assert!(bft.pbft.is_known(&req.d));
    }

    #[tokio::test]
    async fn checkpoint() {
        logger("trace");
        let super::Session {
            mut bft, mut r, mut a, ..
        } = backup();

        let (mine, primary) = request(vec![1; 100].into());

        let pre_prepare = Message::broadcast(0, PrePrepare::new(1, 0, primary.d.clone()));
        accept!(bft.on_message(pre_prepare.pack().unwrap()));
        let assign = Message::broadcast(0, Assign::new(1, primary.m));
        accept!(bft.on_message(assign.pack().unwrap()));

        let state = vec![2; 100];
        let state_digest = ring::digest::digest(&ring::digest::SHA256, &state);
        let checkpoint = Checkpoint::new(10, Bytes::copy_from_slice(state_digest.as_ref()));
        for i in (0..4).filter(|&i| i != 1) {
            let message = Message::broadcast(i, checkpoint.clone());
            bft.on_message(message.pack().unwrap()).await.unwrap();
        }
        let _r = unwrap_poll!(futures_util::poll!(futures_util::stream::StreamExt::next(&mut r)));

        let checkpoint = Message::new(
            0,
            1,
            FullCheckpoint {
                sequence: 10,
                handle: state.into(),
                data: Vec::new().into(),
            },
        );
        let app = async {
            let m = unwrap_poll!(futures_util::poll!(futures_util::stream::StreamExt::next(&mut a))).unwrap();
            match m {
                Command::CheckpointStable { apply: Some(apply), .. } => {
                    apply.tx.send(true).unwrap();
                }
                _ => unreachable!("expected stable"),
            }
        };

        let rep = async {
            bft.on_message(checkpoint.pack().unwrap()).await.unwrap();
        };

        tokio::join!(rep, app);

        accept!(bft.on_request(mine.m));
    }

    fn logger(d: &str) {
        let env_filter = tracing_subscriber::EnvFilter::new(d);
        let _ = tracing_subscriber::fmt().with_env_filter(env_filter).try_init();
    }
}
