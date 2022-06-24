use std::{env, sync::Arc, time::Duration};

use futures_util::{stream, stream::StreamExt};
use rc_application::to_vec;
use themis_client::{Destination, Message, Request};
use themis_core::{app::Client, comms, config, net::RawMessage};
use tokio::{
    sync::Notify,
    time::{timeout, Instant},
};
use tracing_subscriber::{fmt::time::Uptime, EnvFilter};

/// simulation
#[derive(Debug, argh::FromArgs)]
#[argh(subcommand, name = "sim")]
struct Sim {
    /// payload size
    #[argh(option)]
    payload_size: usize,
    /// send interval
    #[argh(option)]
    interval: u64,
}

#[derive(Debug, argh::FromArgs)]
///Baseline client
struct Cli {
    /// config files
    #[argh(option, default = r#""config/".to_string()"#)]
    config: String,
    /// simulation
    #[argh(option)]
    id: u64,
    #[argh(subcommand)]
    sim: Option<Sim>,

    /// mvb: truncate signal list
    #[argh(option)]
    max_signals: Option<usize>,
}

fn make_request(seq: u64, payload_size: usize) -> RawMessage<Client> {
    let mut payload = vec![1u8; payload_size];
    payload[..8].copy_from_slice(&seq.to_le_bytes());
    let command = rc_blockchain::export::Command::transaction(payload.into());
    let command = to_vec(&command).unwrap().into();

    let message = Message::broadcast(0, Request::new(seq, command));
    message.pack().unwrap()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info");
    }

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_timer(Uptime::default())
        .init();

    let cli = argh::from_env();
    println!("{:?}", cli);
    let Cli {
        id,
        config,
        sim,
        max_signals,
    } = cli;

    let mut config = config::load_from_paths(&[config])?;
    config.set("id", id + 100).unwrap();

    if let Some(max_signals) = max_signals {
        config.set("tcn.max_signals", max_signals).unwrap();
    }

    let config = Arc::new(config);

    let (mvb_sender, mut mvb_receiver) = comms::unbounded();
    let mvb_notify = Arc::new(Notify::new());
    if let Some(Sim { payload_size, interval }) = sim {
        tokio::spawn(async move {
            let mut mvb_sender = mvb_sender;
            let mut now = Instant::now();
            let interval = Duration::from_millis(interval);

            loop {
                now += interval;
                tokio::time::sleep_until(now).await;

                let request = make_request(0, payload_size);
                mvb_sender.send(request).await.unwrap();
            }
        });
    } else {
        unimplemented!("mvb is not available");
    }

    let mut client = themis_client::Client::connect(config).await?;
    mvb_notify.notify_one();
    tracing::info!("connected");

    let mut sequence = 0;
    let mut responses = stream::FuturesOrdered::new();
    loop {
        tokio::select! {
            Some(request) = mvb_receiver.next() => {
                let mut request: Message<Request> = request.unpack().unwrap();
                request.source = id;
                request.inner.sequence = sequence;
                sequence += 1;

                let response_future = client.request(request.inner, Destination::Specific(id)).await?;
                responses.push(timeout(Duration::from_secs(30), response_future));
            }
            Some(response) = responses.next() => {
                match response {
                    Ok(Ok(response)) => {
                        tracing::trace!(sequence=response.inner.sequence, "response");
                    }
                    Err(e) => {
                        tracing::error!("timeout: {}", e);
                        break;
                    }
                    Ok(Err(e)) => {
                        tracing::error!("error: {}", e);
                        break;
                    }
                }
            }
            else => {
                tracing::warn!("main loop exit");
                break
            }
        }
    }

    tracing::info!("Exiting");
    tokio::time::sleep(Duration::from_millis(100)).await;

    Ok(())
}
