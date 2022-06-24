use std::{
    convert::TryInto,
    future::{ready, Future},
    pin::Pin,
    str::FromStr,
    sync::Arc,
};

use bytes::Bytes;
use futures_util::{stream::StreamExt, TryFutureExt};
use ring::hmac::HMAC_SHA256;
use tokio::{runtime, spawn, sync::Notify, try_join};
use tracing_subscriber::{
    fmt::{format::FmtSpan, time::Uptime},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter,
};

use rc_blockchain::{
    genesis::genesis_block,
    storage::{Files, StorageMode},
};
use themis_core::{
    app::{Client, Command, Response},
    authentication::{create_crypto_replicas, hmac::HmacFactory, AuthFactory, Authenticator, Cryptography},
    comms::{self as channel, Receiver, Sender},
    config::{self, Config, Peer},
    modules,
    net::{self, connect, Bootstrap, Endpoint, Message, Raw, RawMessage},
};

// #[cfg(feature = "mvb")]
// use rc_application::mvb;
use rc_application::{
    app::{
        self,
        chain::{BlockCutter, Blockchain},
        Railchain,
    },
    request_generator::generate_requests,
};

fn mvb_module(
    mode: Mode,
    config: &Config,
    request_sender: Sender<RawMessage<Client>>,
    notify: Arc<Notify>,
) -> anyhow::Result<Pin<Box<dyn Future<Output = themis_core::Result<()>>>>> {
    #[cfg(feature = "mvb")]
    let _fm = if cfg!(feature = "rc-bft") {
        rc_application::Fm::One
    } else if cfg!(feature = "rc-bft2") {
        rc_application::Fm::Two {
            me: config.get("id").unwrap(),
        }
    } else {
        unreachable!()
    };

    match mode {
        #[cfg(feature = "mvb")]
        Mode::Themis | Mode::Single => {
            unimplemented!("mvb is not available");
        }
        #[cfg(not(feature = "mvb"))]
        Mode::Themis | Mode::Single => {
            panic!("feature mvb is required");
        }
        #[cfg(feature = "osfstream")]
        Mode::Osf => {
            let args = rc_application::osf::Args {
                me: config.get("id")?,
                channel: request_sender,
                notify,
            };

            let config = config.get("osf")?;

            let osf = async {
                tokio::task::spawn_blocking(move || {
                    rc_application::osf::udposf_block(args, config).unwrap();
                    Ok::<_, themis_core::Error>(())
                })
                .await
                .map_err(|e| themis_core::Error::Join {
                    source: e,
                    module: "mvb".to_string(),
                })
                .and_then(std::convert::identity)
            };
            Ok(Box::pin(osf))
        }
        #[cfg(not(feature = "osfstream"))]
        Mode::Osf => {
            panic!("feature osf is required");
        }
        Mode::Program => {
            let me = config.get("id").unwrap();
            let opts = config.get("benchmark").unwrap_or_default();
            Ok(Box::pin(
                generate_requests(me, opts, notify, request_sender).map_err(themis_core::Error::application),
            ))
        }
        Mode::Tcp => Ok(Box::pin(async { Ok(()) })),
    }
}

#[cfg(all(feature = "rc-bft2", not(feature = "rc-bft")))]
use rc_bft2::RcBft2;
#[cfg(all(feature = "rc-bft2", not(feature = "rc-bft")))]
use themis_pbft::messages;

struct ConsensusChannels {
    response_sender: Sender<RawMessage<Client>>,
    requests_receiver: Receiver<RawMessage<Client>>,
    app_in: Sender<Command>,
    app_out: Receiver<Message<Response>>,
    peer_out: Sender<RawMessage<messages::PBFT>>,
    peer_in: Receiver<RawMessage<messages::PBFT>>,
}

fn consensus_module(
    config: &Config,
    mode: Mode,
    ConsensusChannels {
        mut response_sender,
        requests_receiver,
        mut app_in,
        mut app_out,
        peer_out,
        peer_in,
    }: ConsensusChannels,
    verifier: Box<dyn Authenticator<messages::PBFT> + Send + Sync>,
) -> Pin<Box<dyn Future<Output = themis_core::Result<()>> + '_>> {
    let me: u64 = config.get("id").unwrap();
    match mode {
        Mode::Single => Box::pin(async {
            let requests = async move {
                let mut filtered = requests_receiver.filter_map(|r| ready(r.unpack().ok()));
                while let Some(req) = filtered.next().await {
                    let command = Command::Request(req);
                    app_in.send(command).await?;
                }
                Ok::<_, themis_core::Error>(())
            };

            let responses = async move {
                while let Some(response) = app_out.next().await {
                    response_sender.send(response.pack()?).await?;
                }
                Ok::<_, themis_core::Error>(())
            };

            tokio::try_join!(requests, responses)?;
            Ok(())
        }),
        #[cfg(all(feature = "rc-bft2"))]
        _ => {
            let protocol = RcBft2::new(me, peer_out, response_sender, app_in, verifier, config);
            let module = modules::protocol(protocol, peer_in, requests_receiver, app_out, config);
            Box::pin(tokio::task::unconstrained(module))
        }
    }
}

fn peer_module(
    mode: Mode,
    tx: Sender<RawMessage<messages::PBFT>>,
    rx: Receiver<RawMessage<messages::PBFT>>,
    crypto: AuthFactory<messages::PBFT>,
    config: Arc<Config>,
    bootstrap: Option<Bootstrap>,
) -> Pin<Box<dyn Future<Output = themis_core::Result<()>>>> {
    match mode {
        Mode::Single => {
            if let Some(b) = bootstrap {
                b.signal_all()
            }
            Box::pin(async { Ok(()) })
        }
        Mode::Program | Mode::Themis | Mode::Osf | Mode::Tcp => {
            Box::pin(modules::peers(tx, rx, crypto, config, bootstrap))
        }
    }
}

pub async fn rc_clients(
    tx: Sender<Message<Raw<Client>>>,
    rx: Receiver<Message<Raw<Client>>>,
    config: Arc<Config>,
) -> themis_core::Result<()> {
    let id: usize = config.get("id").expect("id");
    let peers: Vec<Peer> = config.get("peers").expect("peers");
    let me = &peers[id];
    let bind = me.bind.as_deref().unwrap_or("::");

    let crypto = HmacFactory::new(id as u64, HMAC_SHA256);

    let group = net::Group::new(rx, tx, crypto.clone(), None);

    if let Ok(push_to) = config.get::<String>("benchmark.exporter_push") {
        tracing::info!("Connecting to exporter: {}", push_to);
        let conn_sender = group.conn_sender();
        spawn(connect(push_to, conn_sender, crypto.clone()));
    }
    let client_ep = Endpoint::bind(id as u64, (bind, me.client_port), group.conn_sender(), crypto).await;

    // listener is fine do shut down
    let _listener = tokio::spawn(client_ep.listen());
    let group = tokio::spawn(group.execute());

    group
        .map_err(|e| themis_core::Error::Join {
            module: "clients".into(),
            source: e,
        })
        .await
}

async fn run(config: Arc<Config>, mode: Mode) -> anyhow::Result<()> {
    let me: usize = config.get("id").expect("id");
    let _keyfile: String = config.get(&format!("peers[{}].private_key", me)).expect("keyfile");
    let peers: Vec<Peer> = config.get("peers").expect("peers");

    let crypto = create_crypto_replicas(&config);
    let verifier = crypto.create_signer(&Bytes::new());

    let notify = Arc::new(Notify::new());
    let bootstrap = Bootstrap::new(peers.len() - 1, notify.clone());

    let (peer_out_tx, peer_out_rx) = channel::unbounded();
    let (peer_in_tx, peer_in_rx) = channel::unbounded();

    let peers = peer_module(mode, peer_in_tx, peer_out_rx, crypto, config.clone(), Some(bootstrap));

    let (response_sender, response_receiver) = channel::unbounded();
    let (request_sender, requests_receiver) = channel::unbounded();

    let mvb = mvb_module(mode, &config, request_sender.clone(), notify)?;
    let clients = rc_clients(request_sender.clone(), response_receiver, config.clone());

    let (app_in_tx, app_in_rx) = channel::unbounded();
    let (app_out_tx, app_out_rx) = channel::unbounded();

    let protocol = consensus_module(
        &config,
        mode,
        ConsensusChannels {
            response_sender,
            requests_receiver,
            app_in: app_in_tx,
            app_out: app_out_rx,
            peer_out: peer_out_tx,
            peer_in: peer_in_rx,
        },
        verifier,
    );

    let railchain_config: app::Config = config.get("railchain")?;
    let keys = railchain_config.load_keys(me.try_into()?)?;
    let cutter = BlockCutter::new(railchain_config);
    let bootstrap = if config.get::<bool>("railchain.reusechain").is_ok() {
        StorageMode::Read
    } else {
        StorageMode::Bootstrap {
            block: genesis_block(),
            clean: true,
        }
    };
    let storage = Files::new(
        config.get("railchain.storage").unwrap_or_else(|_| "./runchain".into()),
        bootstrap,
    );
    let chain = Blockchain::new(storage);
    let checkpoint_interval = config.get("pbft.checkpoint_interval").expect("checkpoint interval");
    let app = Railchain::new(me as u64, checkpoint_interval, cutter, chain, keys);
    let application = modules::application(me as u64, app, app_in_rx, app_out_tx);

    tracing::info!("setup modules");

    let metrics_port: u16 = config
        .get(&format!("peers[{}].prometheus_port", me))
        .expect("prometheus port");
    spawn(themis_core::metrics_server::start_server(metrics_port));
    let state = try_join!(peers, mvb, clients, protocol, application);
    if let Err(e) = state {
        tracing::error!("Fatal: {}", e);
    }

    Ok(())
}

// fn init_tracer<S>(
//     id: u64,
//     address: &str,
// ) -> anyhow::Result<(
//     impl tracing_subscriber::Layer<S>,
//     tracing_appender::non_blocking::WorkerGuard,
// )>
// where
//     S: for<'span> LookupSpan<'span> + Subscriber,
// {
//     let tcp = std::net::TcpStream::connect(address).unwrap();
//     let writer = std::io::BufWriter::new(tcp);

//     let (appender, guard) = tracing_appender::non_blocking(writer);

//     let remote = tracing_subscriber::fmt::layer()
//         .with_timer(Uptime::default())
//         .with_ansi(false)
//         .with_writer(appender)
//         .with_level(false)
//         .with_target(true)
//         .json()
//         .with_span_list(false)
//         .with_current_span(false)
//         .with_span_events(FmtSpan::CLOSE)
//         .flatten_event(true);

//     Ok((remote, guard))
// }

#[derive(Debug, Clone, Copy)]
enum Mode {
    Themis,
    Osf,
    Program,
    #[cfg_attr(not(feature = "mvb"), allow(dead_code))]
    Single,
    Tcp,
}

impl FromStr for Mode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            #[cfg(feature = "mvb")]
            "themis" | "mvb" => Mode::Themis,
            #[cfg(feature = "mvb")]
            "single" => Mode::Single,
            "osf" => Mode::Osf,
            "tcp" => Mode::Tcp,
            "program" => Mode::Program,
            _ => anyhow::bail!("bad mode: {}", s),
        })
    }
}

#[derive(argh::FromArgs)]
///Railchain
struct Cli {
    #[argh(positional)]
    id: Option<u64>,
    /// config files
    #[argh(option, default = r#""config/mvb/".to_string()"#)]
    config: String,
    /// mode of operation
    #[argh(option, default = "Mode::Themis")]
    mode: Mode,
    /// json logging
    #[argh(switch)]
    json_trace: bool,
    /// checkpoint size override
    #[argh(option)]
    checkpoint_size: Option<u64>,
    /// use existing chain on disk
    #[argh(option)]
    resume_chain: Option<u64>,
    /// blockchain storage
    #[argh(option)]
    storage: Option<String>,

    /// signal ratio
    #[argh(option)]
    signal_ratio: Option<f32>,

    /// benchmark payload size
    #[argh(option)]
    bench_payload_size: Option<usize>,
    /// benchmark payload size
    #[argh(option)]
    bench_interval: Option<u64>,

    /// debug: force pbft view change in view
    #[argh(option)]
    pbft_force_in: Option<u64>,
    /// debug: force pbft view change at sequence
    #[argh(option)]
    pbft_force_at: Option<u64>,

    /// mvb: truncate signal list
    #[argh(option)]
    max_signals: Option<usize>,
}

fn main() -> anyhow::Result<()> {
    let Cli {
        id,
        config,
        mode,
        json_trace,
        checkpoint_size,
        resume_chain,
        storage,
        signal_ratio,
        bench_payload_size,
        bench_interval,
        pbft_force_in,
        pbft_force_at,
        max_signals,
    } = argh::from_env();

    let mut config = config::load_from_paths(&[config])?;
    if let Some(id) = id {
        config.set("id", id).unwrap();
    }

    if let Some(checkpoint_size) = checkpoint_size {
        config.set("pbft.checkpoint_interval", checkpoint_size).unwrap();
    }

    if let Some(seq) = resume_chain {
        config.set("pbft.start_sequence", seq).unwrap();
        config.set("railchain.reusechain", true).unwrap();
    }

    if let Some(storage) = storage {
        println!("Setting storage: {}", storage);
        config.set("railchain.storage", storage).unwrap();
    }

    if let Some(signal_ratio) = signal_ratio {
        ratio_signals(signal_ratio, &mut config)
    }

    if let Some(payload_size) = bench_payload_size {
        config.set("benchmark.payload_size", payload_size).unwrap();
    }
    if let Some(interval) = bench_interval {
        config.set("benchmark.interval", interval).unwrap();
    }

    if let (Some(pbft_force_at), Some(pbft_force_in)) = (pbft_force_at, pbft_force_in) {
        config.set("pbft.force_vc_at", pbft_force_at).unwrap();
        config.set("pbft.force_vc_in", pbft_force_in).unwrap();
    };

    if let Some(max_signals) = max_signals {
        config.set("tcn.max_signals", max_signals).unwrap();
    }

    if json_trace {
        let filter = EnvFilter::from_default_env().add_directive("__measure=trace".parse().unwrap());

        #[rustfmt::skip]
        let local = tracing_subscriber::fmt::layer()
            .with_timer(Uptime::default())
            .with_ansi(atty::is(atty::Stream::Stdout))
            .json()
            .with_span_list(false)
            .with_current_span(false)
            .with_span_events(FmtSpan::CLOSE)
            .flatten_event(true);
        tracing_subscriber::registry().with(local).with(filter).init();
    } else {
        let filter = EnvFilter::from_default_env();

        #[rustfmt::skip]
        let local = tracing_subscriber::fmt::layer()
            .with_timer(Uptime::default())
            // .with_span_events(FmtSpan::CLOSE)
            .with_ansi(atty::is(atty::Stream::Stdout));
        tracing_subscriber::registry().with(local).with(filter).init();
    }

    let config = Arc::new(config);
    let runtime = runtime::Builder::new_multi_thread().enable_all().build()?;

    runtime.block_on(run(config, mode))?;

    Ok(())
}

fn ratio_signals(ratio: f32, config: &mut Config) {
    let mut signals: Vec<String> = config.get("tcn.read_signals").unwrap();
    let length = signals.len();
    let length_after = (length as f32 * ratio) as usize;
    signals.truncate(length_after);

    config.set("tcn.read_signals", signals).unwrap();
}
