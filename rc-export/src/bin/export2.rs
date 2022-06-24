use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use prost::Message;
use rc_blockchain::export::BlockVec;
use rc_export::{
    v2::{Exporter2, FetchResponse},
    ClientConfig,
};
use themis_core::config::Peer;
use tokio::io::AsyncWriteExt;
use tracing::Instrument;
use tracing_subscriber::{
    fmt::time::Uptime, prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt, EnvFilter,
};

/// Download block from themis. Export v2
#[derive(Debug, argh::FromArgs)]
struct Cli {
    /// client id
    #[argh(option)]
    id: u64,
    /// export signing key
    #[argh(option)]
    key: String,
    /// config
    #[argh(option)]
    config: String,
    /// lowest block
    #[argh(option)]
    base: u64,
    /// push mode
    #[argh(option)]
    listen: Option<String>,
    /// write blocks to directory
    #[argh(option)]
    output: Option<PathBuf>,
    /// write data to postgres
    #[argh(option)]
    postgres: Option<String>,
    /// write data to Victoria Metrics
    #[argh(option)]
    vic_addr: Option<String>,
    /// the XML Header File for victoria export
    #[argh(option)]
    vic_header: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // tracing_subscriber::fmt::init();

    // let filter = EnvFilter::from_default_env().add_directive("__measure=trace".parse().unwrap());
    // let filter = EnvFilter::from_default_env()
    //     .add_directive("info".parse().unwrap())
    //     .add_directive("rc-export=trace".parse().unwrap())
    //     .add_directive("themis_core::debug".parse().unwrap());

    #[rustfmt::skip]
    let local = tracing_subscriber::fmt::layer()
        .with_timer(Uptime::default())
        // .with_ansi(atty::is(atty::Stream::Stdout))
        .with_ansi(false);
    // .json()
    // .with_span_list(false)
    // .with_current_span(false)
    // .with_span_events(FmtSpan::CLOSE)
    // .flatten_event(true);
    tracing_subscriber::registry()
        .with(local)
        .with(EnvFilter::from_default_env())
        .init();

    #[allow(unused_variables)]
    let Cli {
        id,
        key,
        config,
        base,
        listen,
        output,
        postgres,
        vic_addr,
        vic_header,
    } = argh::from_env();

    if let Some(output) = &output {
        if !output.exists() {
            anyhow::bail!("Output Directory does not exist");
        }
    }

    let config = themis_core::config::load_from_paths(&[config]).context("config")?;
    let peers: Vec<Peer> = config.get("peers").context("peers config")?;

    let client_config = ClientConfig::new(id, key, listen);

    let mut exporter = Exporter2::connect(client_config, peers).await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    async {
        let segment: FetchResponse = exporter
            .fetch_blocks(base)
            .instrument(tracing::info_span!(target: "__measure::export::fetch", parent: None, "fetch"))
            .await
            .context("fetch blocks")?;

        let last_header = segment.blocks.last().map(|b| b.header().clone()).expect("no header");

        exporter
            .delete_blocks(last_header)
            .instrument(tracing::info_span!(target: "__measure::export::delete", parent: None, "delete"))
            .await
            .context("delete")?;

        #[cfg(feature = "victoria")]
        if let (Some(addr), Some(header)) = (vic_addr, vic_header) {
            use rc_victoria::VicotriaOpts;

            let opts = VicotriaOpts {
                job: "export",
                instance: "export",
                victoria: addr,
                channel_file: header,
            };
            if let Err(e) = rc_victoria::write_blocks_to_victoria(opts, segment.blocks.iter()).await {
                tracing::error!(%e, "victoria export failed");
            };
        }

        if let Some(output) = &output {
            let blocks = BlockVec {
                blocks: segment.blocks,
                proof: segment.proof.quorum.unwrap().messages,
            };
            let filename = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
            let filename = filename.as_millis();
            let mut file = tokio::fs::File::create(output.join(format!("export-{}.bin", filename)))
                .await
                .unwrap();
            let mut buffer = Vec::with_capacity(blocks.encoded_len());
            blocks.encode_length_delimited(&mut buffer).unwrap();
            file.write_all(&buffer).await.unwrap();
        }

        Ok(())
    }
    .instrument(tracing::info_span!(target: "__measure::export", parent: None, "export"))
    .await
}
