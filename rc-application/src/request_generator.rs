use std::{borrow::Cow, fmt::Display, path::Path, sync::Arc, time::Duration};

use bytes::Bytes;
use rc_blockchain::{export::Command, to_vec};
use rc_broadcaster::program::{load_program, Kind};
use themis_core::{
    app::{Client, Request},
    comms::Sender,
    net::{Message, RawMessage},
};
use tokio::{io::AsyncReadExt, net::TcpListener, sync::Notify};

use super::timestamp;

mod d {
    use super::Options;
    use std::{borrow::Cow, path::Path};

    macro_rules! d {
        ($name:ident, $t:ty) => {
            pub fn $name() -> $t {
                Options::default().$name
            }
        };
    }

    d!(interval, u64);
    d!(payload_size, usize);
    d!(program, Cow<'static, Path>);
    d!(request_limit, usize);
    d!(source_is_me, bool);
    d!(wait_control_port, bool);
}

#[derive(Debug, serde::Deserialize)]
pub struct Options {
    #[serde(default = "d::interval")]
    interval: u64,
    #[serde(default = "d::payload_size")]
    payload_size: usize,
    #[serde(default = "d::program")]
    program: Cow<'static, Path>,
    #[serde(default = "d::request_limit")]
    request_limit: usize,
    #[serde(default = "d::source_is_me")]
    source_is_me: bool,
    #[serde(default = "d::wait_control_port")]
    wait_control_port: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            interval: 64,
            payload_size: 1024,
            program: Cow::Borrowed("rc-broadcaster/programs/single.ron".as_ref()),
            request_limit: 0,
            source_is_me: true,
            wait_control_port: true,
        }
    }
}

fn make_payload(sequence: u64, payload_size: usize) -> Bytes {
    let payload_size = payload_size.max(16);

    let payload: Vec<u8> = IntoIterator::into_iter(sequence.to_be_bytes())
        .cycle()
        .take(payload_size)
        .collect();

    payload.into()
}

fn make_apayload(sequence: u64, payload_size: usize, i: u64) -> Bytes {
    let payload_size = payload_size.max(16);

    let mut payload: Vec<u8> = IntoIterator::into_iter(sequence.to_be_bytes())
        .cycle()
        .take(payload_size)
        .collect();

    payload[8..16].copy_from_slice(&i.to_be_bytes());

    payload.into()
}

fn make_cpayload(sequence: u64, payload_size: usize) -> Bytes {
    let payload_size = payload_size.max(16);

    let payload: Vec<u8> = IntoIterator::into_iter(sequence.to_le_bytes())
        .cycle()
        .take(payload_size)
        .collect();

    payload.into()
}

pub async fn control_port() {
    let server = TcpListener::bind(("0.0.0.0", 9999)).await.unwrap();
    tracing::info!("control");
    let (mut stream, addr) = server.accept().await.unwrap();
    tracing::info!(?addr, "control conn");

    let mut buf = [0u8; 2];
    stream.read_exact(&mut buf).await.unwrap();

    assert_eq!(&buf, b"go");
    tracing::info!("go");
}

pub async fn generate_requests(
    me: u64,
    config: Options,
    bootstrap: Arc<Notify>,
    mut requests: Sender<RawMessage<Client>>,
) -> Result<(), Error> {
    tracing::info!(?config, "");
    let program = load_program(&config.program)?;

    tracing::info!(?program, "");

    let sleep_duration = Duration::from_millis(config.interval);
    bootstrap.notified().await;
    if config.wait_control_port {
        control_port().await;
    }

    let mut interval = tokio::time::interval(sleep_duration);

    let iter: Box<dyn Iterator<Item = Kind> + Send + Sync> = if config.request_limit > 0 {
        Box::new(program.into_iter().cycle().take(config.request_limit))
    } else {
        Box::new(program.into_iter().cycle())
    };

    let source = if config.source_is_me { me } else { 100 };

    for (i, kind) in iter.enumerate() {
        let _tick = interval.tick().await;
        // let seq = timestamp();
        let seq = i as u64;

        let request = Command::transaction(make_payload(i as u64, config.payload_size));

        let message = match kind {
            Kind::Normal => Message::new(source, me, Request::new(seq, to_vec(&request).unwrap().into())),
            Kind::Skip { to } => {
                if to == me {
                    continue;
                }
                Message::new(source, me, Request::new(seq, to_vec(&request).unwrap().into()))
            }
            Kind::Corrupt { value: _value, to } => {
                if to != me {
                    continue;
                }
                let request = Command::transaction(make_cpayload(seq, config.payload_size));
                Message::new(source, me, Request::new(seq, to_vec(&request).unwrap().into()))
            }
            Kind::Additional { to, n } => {
                if to == me {
                    for i in 0..n {
                        let value = make_apayload(seq, config.payload_size, i);
                        let request = Command::transaction(value);
                        let msg = Message::new(source, me, Request::new(timestamp(), to_vec(&request).unwrap().into()));

                        if (requests.send(msg.pack().unwrap()).await).is_err() {
                            break;
                        };
                    }
                }
                Message::new(source, me, Request::new(seq, to_vec(&request).unwrap().into()))
            }
        };

        tracing::trace!("new request");
        // message.trace();
        if (requests.send(message.pack().unwrap()).await).is_err() {
            tracing::warn!("sender closed");
            break;
        };
    }

    Ok(())
}

pub async fn spawn_generate_requests(
    me: u64,
    config: Options,
    bootstrap: Arc<Notify>,
    requests: Sender<RawMessage<Client>>,
) -> Result<(), Error> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let _ = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(async { generate_requests(me, config, bootstrap, requests).await });
        let _ = tx.send(result);
    });
    rx.await.map_err(|e| anyhow::anyhow!(e).into()).and_then(|r| r)
}

#[derive(Debug)]
pub struct Error {
    inner: anyhow::Error,
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}

impl std::error::Error for Error {}

impl From<anyhow::Error> for Error {
    fn from(inner: anyhow::Error) -> Self {
        Self { inner }
    }
}
