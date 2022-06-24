use std::{convert::TryInto, sync::Arc};

use bytes::Bytes;

use themis_pbft::PBFT;
use tokio::{runtime, try_join};

use rc_blockchain::{
    genesis::genesis_block,
    storage::{Files, StorageMode},
};
use themis_core::{
    authentication::{create_crypto_replicas, Cryptography},
    comms as channel,
    config::{self, Config, Peer},
    modules,
};

use rc_application::app::{
    self,
    chain::{BlockCutter, Blockchain},
    Railchain,
};
use tracing_subscriber::{
    fmt::{format::FmtSpan, time::Uptime},
    prelude::__tracing_subscriber_SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter,
};

async fn run_tcp_mode(config: Arc<Config>) -> anyhow::Result<()> {
    let me: usize = config.get("id").expect("id");
    let _keyfile: String = config.get(&format!("peers[{}].private_key", me)).expect("keyfile");
    let _peers: Vec<Peer> = config.get("peers").expect("peers");

    let crypto = create_crypto_replicas(&config);
    let verifier = crypto.create_signer(&Bytes::new());

    let peer_out = channel::unbounded();
    let peer_in = channel::unbounded();
    let peers = modules::peers(peer_in.0, peer_out.1, crypto, config.clone(), None);

    let (response_sender, response_receiver) = channel::unbounded();
    let (request_sender, requests_receiver) = channel::unbounded();
    let clients = modules::clients(request_sender.clone(), response_receiver, config.clone());

    let app_in = channel::unbounded();
    let app_out = channel::unbounded();

    let protocol = PBFT::new(me as u64, peer_out.0, response_sender, app_in.0, verifier, &config);
    let protocol = modules::protocol(protocol, peer_in.1, requests_receiver, app_out.1, &config);

    let railchain_config: app::Config = config.get("railchain")?;
    let keys = railchain_config.load_keys(me.try_into()?)?;
    let cutter = BlockCutter::new(railchain_config);
    let storage = Files::new(
        "./runchain/".into(),
        StorageMode::Bootstrap {
            block: genesis_block(),
            clean: true,
        },
    );
    let chain = Blockchain::new(storage);

    let checkpoint_interval = config.get("pbft.checkpoint_interval").expect("checkpoint interval");
    let app = Railchain::new(me as u64, checkpoint_interval, cutter, chain, keys);
    let application = modules::application(me as u64, app, app_in.1, app_out.0);

    tracing::info!("setup modules");

    let state = try_join!(peers, clients, protocol, application);
    if let Err(e) = state {
        eprintln!("Fatal: {}", e);
    } else {
        eprintln!("Replica shut down");
    }

    Ok(())
}

#[derive(Debug, argh::FromArgs)]
/// Baseline Server
struct Cli {
    #[argh(positional)]
    id: u64,
    /// config files
    #[argh(option, default = r#""config/*".to_string()"#)]
    config: String,
    /// json logging
    #[argh(switch)]
    json_trace: bool,

    /// force view change at sequence
    #[argh(option)]
    pbft_force_at: Option<u64>,
    /// force view change in view
    #[argh(option)]
    pbft_force_in: Option<u64>,
}

fn main() -> anyhow::Result<()> {
    let cli = argh::from_env();
    println!("{:?}", cli);
    let Cli {
        id,
        config,
        json_trace,
        pbft_force_at,
        pbft_force_in,
    } = cli;

    let mut config = config::load_from_paths(&[config])?;
    config.set("id", id).unwrap();

    if let (Some(pbft_force_at), Some(pbft_force_in)) = (pbft_force_at, pbft_force_in) {
        config.set("pbft.force_vc_at", pbft_force_at).unwrap();
        config.set("pbft.force_vc_in", pbft_force_in).unwrap();
    };

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

        tracing_subscriber::fmt()
            .with_timer(Uptime::default())
            .with_ansi(atty::is(atty::Stream::Stdout))
            .with_env_filter(filter)
            .init();
    }

    let config = Arc::new(config);
    let runtime = runtime::Builder::new_multi_thread().enable_all().build()?;

    runtime.block_on(async move { run_tcp_mode(config).await })?;

    Ok(())
}
