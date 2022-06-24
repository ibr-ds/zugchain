use std::{
    convert::TryInto,
    io::ErrorKind,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::Bytes;
use osf::datagram::OsfDatagramStream;
use rc_blockchain::export::{transaction::TxType, Command, Transaction};
use themis_core::{
    app::{Client, Request},
    comms::Sender,
    net::{Message, RawMessage},
};
use tokio::{net::UdpSocket, sync::Notify, time::timeout};

use crate::timestamp;

pub struct Args {
    pub me: u64,
    pub channel: Sender<RawMessage<Client>>,
    pub notify: Arc<Notify>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct UdpOsfConfig {
    port: u16,
    packet_buffer: usize,
    request_buffer: usize,
}

pub fn udposf_block(args: Args, config: UdpOsfConfig) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(udposf_main(args, config))
}

pub async fn udposf_main(mut args: Args, config: UdpOsfConfig) -> anyhow::Result<()> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    socket.set_nonblocking(true)?;
    socket.set_recv_buffer_size(512000).unwrap();
    socket.set_reuse_address(true)?;
    let addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), config.port);
    let addr = addr.into();
    socket.bind(&addr)?;
    let socket: std::net::UdpSocket = socket.into();
    let socket = UdpSocket::from_std(socket)?;

    println!("listening for udp on port {}", config.port);
    let mut packet_buffer = vec![0; config.packet_buffer];
    let mut block_buffer = Vec::with_capacity(config.request_buffer);
    let mut stream = OsfDatagramStream::new();
    let mut now = Instant::now();

    loop {
        socket.readable().await?;
        let elapsed = now.elapsed().as_millis() as u64;
        tracing::debug!(elapsed, "readable");

        let mut packets = 0;
        loop {
            let (size, _addr) = match socket.try_recv_from(&mut packet_buffer) {
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    match timeout(Duration::from_millis(5), socket.readable()).await {
                        Ok(_) => continue,
                        Err(_) => break,
                    };
                }
                e => e.unwrap(),
            };

            record_packet();
            let r = match stream.feed(&packet_buffer[..size]) {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("{}", e);
                    continue;
                }
            };
            match r {
                osf::datagram::Step::Blocks(block_data) => {
                    packets += 1;
                    block_buffer.extend_from_slice(block_data);
                }
                osf::datagram::Step::FullHeader { bytes } => {
                    args.notify.notify_one();
                    send_header_block(args.me, &mut args.channel, bytes).await?;
                    break;
                }
                osf::datagram::Step::HeaderFrame => {}
                osf::datagram::Step::HeaderStart(packet_count) => {
                    record_header_packets(packet_count);
                    break;
                }
            }
        }

        if !block_buffer.is_empty() {
            let bytes = block_buffer.len();
            tracing::debug!(packets, bytes, "request");
            send_data_block(
                args.me,
                &mut args.channel,
                std::mem::replace(&mut block_buffer, Vec::with_capacity(config.request_buffer)),
            )
            .await?;
            block_buffer.clear();
        }
        now = Instant::now();
    }
}

async fn send_data_block(me: u64, channel: &mut Sender<RawMessage<Client>>, data: Vec<u8>) -> anyhow::Result<()> {
    record_request(data.len());
    let command = Command::transaction2(Transaction {
        payload: data.to_owned().into(),
        tx_type: TxType::Data.into(),
    });
    let payload: Bytes = rc_blockchain::to_vec(&command).unwrap().into();
    let len = payload.len();
    tracing::trace!(target: "__measure::payload_size", parent: None, pl=len, "plsize");
    tracing::debug!(size = len, "request");
    let message = Message::broadcast(me, Request::new(timestamp(), payload));
    let message = message.pack()?;

    channel.send(message).await?;
    Ok(())
}

async fn send_header_block(me: u64, channel: &mut Sender<RawMessage<Client>>, data: Vec<u8>) -> anyhow::Result<()> {
    record_request(data.len());
    let command = Command::transaction2(Transaction {
        payload: data.into(),
        tx_type: TxType::Config.into(),
    });
    let payload: Bytes = rc_blockchain::to_vec(&command).unwrap().into();
    let len = payload.len();
    tracing::trace!(target: "__measure::payload_size", parent: None, pl=len, "plsize");
    tracing::debug!(size = len, "request");
    let message = Message::broadcast(me, Request::new(timestamp(), payload));
    let message = message.pack()?;
    channel.send(message).await?;
    Ok(())
}

#[cfg(feature = "metrics")]
mod metrics {
    use lazy_static::lazy_static;
    use prometheus::{register_int_counter, register_int_gauge, IntCounter, IntGauge};

    lazy_static! {
        pub static ref UDP_PACKETS: IntCounter = register_int_counter!(
            "raily_osfstream_udp_packets",
            "number of udp packets received over osfstream"
        )
        .unwrap();
        pub static ref REQUEST_SIZE_TOTAL: IntCounter = register_int_counter!(
            "raily_osfstream_request_size_total",
            "Total size of all requests created by osfstream"
        )
        .unwrap();
        pub static ref REQUEST_COUNT: IntCounter = register_int_counter!(
            "raily_osfstream_request_count",
            "Number of requests created by osfstream"
        )
        .unwrap();
        pub static ref OSF_CHANNELS: IntGauge = register_int_gauge!(
            "raily_osfstream_osf_channels",
            "Number of channels currently in osfstream"
        )
        .unwrap();
        pub static ref OSF_HEADER_PACKETS: IntGauge =
            register_int_gauge!("raily_osfstream_header_packets", "Number packets in the current header").unwrap();
    }
}

pub fn record_request(size: usize) {
    #[cfg(feature = "metrics")]
    {
        if let Ok(size) = size.try_into() {
            metrics::REQUEST_SIZE_TOTAL.inc_by(size);
            metrics::REQUEST_COUNT.inc();
        }
    }
}

pub fn record_packet() {
    #[cfg(feature = "metrics")]
    {
        metrics::UDP_PACKETS.inc();
    }
}

pub fn record_header(num_channels: usize) {
    #[cfg(feature = "metrics")]
    {
        if let Ok(num_channels) = num_channels.try_into() {
            metrics::OSF_CHANNELS.set(num_channels);
        }
    }
}

pub fn record_header_packets(num_packets: u32) {
    #[cfg(feature = "metrics")]
    {
        if let Ok(num_packets) = num_packets.try_into() {
            metrics::OSF_HEADER_PACKETS.set(num_packets);
        }
    }
}
