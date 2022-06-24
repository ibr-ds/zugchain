use anyhow::{Context, Result};
use bytes::Bytes;
use fs::File;
use futures_util::{
    future::try_join_all,
    stream::{iter, Stream, StreamExt},
};
use program::{load_program, Kind};
use ring::hmac::HMAC_SHA256;
use std::{env, fmt::Debug, fs, future::Future, path::PathBuf, pin::Pin, sync::Arc, time::Duration};
use structopt::StructOpt;
use themis_core::{
    app::{Client, Request, Response},
    authentication::hmac::HmacFactory,
    comms::{Receiver, Sender},
    config::{load_from_paths, Config, Peer},
    net::{Message, RawMessage},
    protocol::Match,
};
use tokio::{
    io::AsyncBufReadExt,
    process::{Child, Command},
    sync::Notify,
    time::{sleep, sleep_until, timeout},
};
use tracing_subscriber::EnvFilter;

pub mod program;

#[derive(Debug)]
enum System {
    Debug,
    Release,
}

impl std::str::FromStr for System {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "debug" | "d" => Ok(Self::Debug),
            "release" | "r" => Ok(Self::Release),
            _ => anyhow::bail!("unknown: {}", s),
        }
    }
}

#[derive(Debug, StructOpt)]
struct Cli {
    #[structopt(long, default_value = "config/localhost/*")]
    config: String,
    #[structopt(subcommand)]
    kind: Sub,
    #[structopt(long)]
    system: Option<System>,
}

#[derive(Debug, StructOpt)]
enum Sub {
    Test(Test),
    Benchmark(Benchmark),
}

#[derive(Debug, StructOpt)]
struct Test {
    #[structopt(long)]
    program: Option<PathBuf>,
    #[structopt(long)]
    step: bool,
    #[structopt(long)]
    wait: bool,
    #[structopt(long)]
    fm1: bool,
}

#[derive(Debug, StructOpt)]
enum BenchmarkKind {
    Regular {
        id: u64,
    },
    ViewChange {
        #[structopt(long)]
        vc_freq: u64,
        #[structopt(long, default_value = "0")]
        offset: u64,
        id: u64,
    },
}

#[derive(Debug, StructOpt)]
struct Benchmark {
    #[structopt(subcommand)]
    kind: BenchmarkKind,
    #[structopt(long)]
    interval: u64,
    #[structopt(long, default_value = "100")]
    payload_size: usize,
    #[structopt(long)]
    duration: Option<u64>,
    #[structopt(long)]
    no_broadcast: bool,
}

async fn connect(config: &Config) -> Result<(Sender<RawMessage<Client>>, Receiver<RawMessage<Client>>)> {
    let peers: Vec<Peer> = config.get("peers")?;
    let crypto = HmacFactory::new(config.get("id").unwrap(), HMAC_SHA256);
    let (requests_sender, requests_receiver) = themis_core::comms::unbounded();
    let (response_sender, response_receiver) = themis_core::comms::unbounded();

    let notify = Arc::new(Notify::new());
    let bootstrap = themis_core::net::Bootstrap::new(peers.len(), notify.clone());

    let group = themis_core::net::Group::new(requests_receiver, response_sender, crypto.clone(), Some(bootstrap));
    let new_conns_sender = group.conn_sender().clone();

    tokio::spawn(group.execute());
    try_join_all(peers.iter().map(|peer| {
        themis_core::net::connect(
            (peer.host.as_str(), peer.client_port),
            new_conns_sender.clone(),
            crypto.clone(),
        )
    }))
    .await?;
    notify.notified().await;
    Ok((requests_sender, response_receiver))
}

async fn test_main(
    config: Config,
    Test {
        program,
        step,
        wait,
        fm1,
    }: Test,
) -> Result<()> {
    let program = match program {
        None => vec![Kind::Normal],
        Some(path) => load_program(&path).with_context(|| path.display().to_string()).unwrap(),
    };
    tracing::info!("{:?}", program);

    sleep(Duration::from_secs(2)).await;

    let expected_replies = program::count_replies_fm2(&program);
    let program = program::create_program(!fm1, program, 4);

    let (mut requests_sender, response_receiver) = connect(&config).await?;

    tracing::info!("Connected to System.");

    let write_task = Box::pin(async {
        for req_set in program {
            if step {
                let mut buf = String::new();
                let stdin = tokio::io::stdin();
                let mut stdin = tokio::io::BufReader::new(stdin);
                stdin.read_line(&mut buf).await.unwrap();
            } else if wait {
                sleep(Duration::from_millis(64)).await;
            }
            for req in req_set {
                requests_sender.send(req.pack().unwrap()).await?;
            }
        }
        Ok::<_, anyhow::Error>(())
    });

    let read_task = Box::pin(async {
        timeout(
            Duration::from_secs(10),
            process_replies(response_receiver, expected_replies),
        )
        .await??;
        Ok::<_, anyhow::Error>(())
    });

    let _results = tokio::try_join!(read_task, write_task)?;

    Ok(())
}

fn make_request(seq: u64, payload_size: usize) -> RawMessage<Client> {
    let mut payload = vec![1u8; payload_size];
    payload[..8].copy_from_slice(&seq.to_le_bytes());
    let command = rc_blockchain::export::Command::transaction(payload.into());
    let command = rc_blockchain::to_vec(&command).unwrap().into();

    let message = Message::broadcast(0, Request::new(seq, command));
    message.pack().unwrap()
}

fn make_targeted_request(target: u64, seq: u64, payload_size: usize) -> RawMessage<Client> {
    let mut request = make_request(seq, payload_size);
    request.destination = target;
    request
}

fn make_request_trigger_vc(seq: u64, primary: u64) -> Vec<RawMessage<Client>> {
    let mut payload = vec![0u8; 100];
    payload[..8].copy_from_slice(&seq.to_le_bytes());
    let command = rc_blockchain::export::Command::transaction(payload.into());
    let command: Bytes = rc_blockchain::to_vec(&command).unwrap().into();

    (0..4)
        .filter(|&i| i != primary)
        .map(|i| Message::new(0, i, Request::new(seq, command.clone())))
        .map(|m| m.pack().unwrap())
        .collect()
}

async fn benchmark_main(
    config: Config,
    Benchmark {
        kind,
        interval,
        duration,
        payload_size,
        no_broadcast,
    }: Benchmark,
) -> Result<()> {
    sleep(Duration::from_secs(2)).await;
    let (mut requests_sender, mut response_receiver) = connect(&config).await?;
    tracing::info!("connected");

    let me: u64 = config.get("id").unwrap();

    let write_task: Pin<Box<dyn Future<Output = Result<()>>>> = match kind {
        BenchmarkKind::Regular { id: _id } => Box::pin(async {
            let mut next = tokio::time::Instant::now();
            for seq in 0.. {
                next += Duration::from_millis(interval);
                sleep_until(next).await;

                let message = if no_broadcast {
                    make_targeted_request(me, seq, payload_size)
                } else {
                    make_request(seq, payload_size)
                };

                requests_sender.send(message).await?;
            }
            Ok::<_, anyhow::Error>(())
        }),
        BenchmarkKind::ViewChange {
            vc_freq,
            offset,
            id: _id,
        } => Box::pin(async move {
            let mut primary = 0u64;
            for seq in 0..offset {
                sleep(Duration::from_millis(interval)).await;
                tracing::info!("Normal Request");
                requests_sender.send(make_request(seq, payload_size)).await?;
            }
            for seq in 0.. {
                sleep(Duration::from_millis(interval)).await;
                if seq % vc_freq == 0 {
                    tracing::info!("View Change to primary {}", primary);
                    let reqs = make_request_trigger_vc(seq + offset, primary);
                    requests_sender.send_all(iter(reqs)).await?;
                    primary += 1;
                    primary %= 4;
                } else {
                    tracing::info!("Normal Request");
                    requests_sender.send(make_request(seq + offset, payload_size)).await?;
                }
            }
            Ok::<_, anyhow::Error>(())
        }),
    };

    let read_task = Box::pin(async {
        loop {
            tokio::select! {
                Some(response) = response_receiver.next() => {
                    let response: Message<Response> = response.unpack().unwrap();
                    tracing::debug!(sequence=response.inner.sequence, "response")
                }
                _timeout = sleep(Duration::from_secs(10)) => {
                    anyhow::bail!("Responses timeout")
                }
            }
        }
        #[allow(unreachable_code)] // type inference
        Ok::<_, anyhow::Error>(())
    });

    if let Some(duration) = duration {
        tokio::select! {
            result = async { tokio::try_join!(read_task, write_task) } => {
                result?;
            }
            _timeout = sleep(Duration::from_millis(duration)) => {
                tracing::info!("Benchmark terminated");
            }
        }
    } else {
        tokio::try_join!(read_task, write_task)?;
    }

    Ok(())
}

pub async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::from_args();
    tracing::info!("{:?}", cli);
    let Cli { config, kind, system } = cli;

    let mut config = load_from_paths(&[config]).context("load config")?;
    config.set("id", 0).expect("set own id");

    let system_task = tokio::spawn(async move {
        let system = match system {
            Some(System::Release) => "release",
            Some(System::Debug) => "debug",
            None => return Ok(()),
        };

        let mut log_base_path: PathBuf = env::var("CARGO_MANIFEST_DIR").expect("cargo sets this").into();
        log_base_path.push("logs");
        fs::create_dir_all(&log_base_path).unwrap();

        let program_name = "none";
        let mut procs = Vec::new();
        for i in 0..4 {
            let log_file = format!("broadcaster-{}-{}.log", program_name, i);
            let log_file = log_base_path.join(log_file);
            let out = File::create(log_file).unwrap();

            let mut child = Command::new(format!("./target/{}/railchain", system));
            child
                .args(&[
                    "--mode",
                    "tcp",
                    "--config",
                    "./config/localhost/*",
                    // "--tracing",
                    &format!("{}", i),
                ])
                .stdout(out.try_clone().unwrap())
                .stderr(out)
                .kill_on_drop(true);
            if let Ok(rust_log) = std::env::var("RUST_LOG") {
                child.env("RUST_LOG", rust_log);
            }
            let child = child.spawn().unwrap();
            procs.push(child);
            // tracing::info!("Launched {}", i);
        }

        if let Err(e) = try_join_all(procs.iter_mut().map(Child::wait)).await {
            tracing::error!("themis shut down: {}", e);
            std::process::exit(-1);
        }
        Ok::<_, anyhow::Error>(())
    });

    let _aborter = tokio::spawn(async move {
        match system_task.await {
            Err(e) => {
                tracing::error!("system: {}", e);
                std::process::exit(-1);
            }
            Ok(Err(e)) => {
                tracing::error!("system: {}", e);
                std::process::exit(-1);
            }
            Ok(Ok(_)) => {}
        }
    });

    match kind {
        Sub::Test(test) => test_main(config, test).await?,
        Sub::Benchmark(bench) => benchmark_main(config, bench).await?,
    }

    sleep(Duration::from_secs(1)).await;

    Ok(())
}

async fn read_response<T>(stream: &mut T) -> anyhow::Result<Message<Response>>
where
    T: Stream<Item = RawMessage<Client>> + Unpin,
{
    let next = stream.next().await.ok_or_else(|| anyhow::anyhow!("none"))?;
    let next = next.unpack()?;
    Ok(next)
}

async fn process_replies<T>(mut streams: T, expected: usize) -> anyhow::Result<()>
where
    T: Stream<Item = RawMessage<Client>> + Unpin,
{
    let mut replies = vec![Vec::new(); expected];
    let mut completed = 0;

    tracing::info!("Expected replies: {}", expected);

    loop {
        let reply = read_response(&mut streams).await?;
        tracing::info!("{:?}", reply);
        let seq = reply.inner.sequence;

        replies[seq as usize].push(reply);

        let replies = &replies[completed];

        if replies.len() < 4 {
            continue;
        } else if check_replies(replies) {
            completed += 1;
            tracing::info!("All replies equal!");
        } else {
            tracing::info!("Replies don't match");
            tracing::info!("{:#?}", &replies[completed]);
            anyhow::bail!("Bad replies at sequence {}", completed);
        }
        if completed == expected {
            break;
        }
    }
    Ok(())
}

fn check_replies(replies: &[Message<Response>]) -> bool {
    if replies.is_empty() {
        return false;
    }
    let first = &replies[0];
    replies[1..].iter().all(|r| r.matches(first))
}

#[cfg(test)]
mod test {
    use crate::program::Kind;

    #[test]
    fn example_program() {
        let program = vec![
            Kind::Normal,
            Kind::Skip { to: 1 },
            Kind::Corrupt {
                value: "bad".to_string(),
                to: 2,
            },
            Kind::Additional { to: 3, n: 1 },
        ];

        let program_str = ron::to_string(&program).unwrap();

        println!("{}", program_str);
    }
}
