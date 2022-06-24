use bytes::Buf;

use crate::{
    block::{self, OsfBlock, OsfBlockCommon},
    header::Channel,
};

#[derive(Debug)]
pub enum OsfPacketId {
    HeaderHeader,
    HeaderData,
    Data,
}

impl OsfPacketId {
    fn read<B: Buf>(buffer: &mut B) -> Option<OsfPacketId> {
        let chunk = buffer.chunk();
        let id = if chunk.starts_with(b"OSF40") {
            Some(OsfPacketId::HeaderHeader)
        } else if chunk.starts_with(b"OSF41") {
            Some(OsfPacketId::HeaderData)
        } else if chunk.starts_with(b"OSF42") {
            Some(OsfPacketId::Data)
        } else {
            None
        };
        buffer.advance(5);
        id
    }
}

#[derive(Debug)]
pub struct OsfPacketInfo {
    id: OsfPacketId,
    timestamp: u64,
    #[allow(dead_code)]
    data_size: u16,
}

impl OsfPacketInfo {
    fn read<B: Buf>(buffer: &mut B) -> Option<Self> {
        let id = OsfPacketId::read(buffer)?;
        let timestamp = buffer.get_u64();
        // don't need this UDP packets know how large they are
        let data_size = buffer.get_u16();

        Some(OsfPacketInfo {
            id,
            timestamp,
            data_size,
        })
    }
}

enum State {
    Header {
        /// Number of header packets we still need to assemble a header
        current_header_packets: usize,
        /// Header chunks
        channel_data: Box<[Box<[u8]>]>,
    },
    Data {
        // channels: Vec<Channel>,
    },
}
pub struct OsfDatagramStream {
    /// We only accept data packets with this timestamp because we need the Header to interpret the block data
    header_timestamp: u64,
    state: State,
}

pub enum Step<'data> {
    Blocks(&'data [u8]),
    HeaderFrame,
    HeaderStart(u32),
    FullHeader { bytes: Vec<u8> },
}

impl Default for OsfDatagramStream {
    fn default() -> Self {
        Self::new()
    }
}

impl OsfDatagramStream {
    pub fn new() -> Self {
        Self {
            header_timestamp: 0,
            state: State::Header {
                current_header_packets: 0,
                channel_data: Box::new([]) as _,
            },
        }
    }

    fn header_header(&mut self, info: OsfPacketInfo, mut packet: &[u8]) -> Result<u32, Error> {
        let header_packet_count = packet.get_u32();
        match self.state {
            State::Data { .. } | State::Header { .. } if info.timestamp > self.header_timestamp => {
                self.state = State::Header {
                    current_header_packets: 0,
                    channel_data: vec![Box::new([]) as _; header_packet_count as usize].into(),
                };
                self.header_timestamp = info.timestamp;
            }
            _ => {
                return Err(Error::StaleHeader {
                    expected: self.header_timestamp,
                    actual: info.timestamp,
                });
            }
        }

        tracing::info!("new header {}: {} chunks", info.timestamp, header_packet_count);
        Ok(header_packet_count)
    }

    fn header_data<'data>(&mut self, info: OsfPacketInfo, mut packet: &'data [u8]) -> Result<Step<'data>, Error> {
        if info.timestamp != self.header_timestamp {
            return Err(Error::StaleHeader {
                actual: info.timestamp,
                expected: self.header_timestamp,
            });
        }

        if let State::Header {
            channel_data,
            current_header_packets,
        } = &mut self.state
        {
            let index = packet.get_u32() as usize;
            if index > channel_data.len() {
                return Err(Error::HeaderRange { found: index });
            }
            if channel_data[index].is_empty() {
                println!("header fragment {}", index);
                channel_data[index] = packet.to_owned().into();
                *current_header_packets += 1;
                if *current_header_packets == channel_data.len() {
                    let mut header_data: Vec<u8> = Vec::with_capacity(channel_data.iter().map(|b| b.len()).sum());
                    for fragment in channel_data.iter() {
                        header_data.extend_from_slice(fragment);
                    }

                    self.state = State::Data {};
                    return Ok(Step::FullHeader { bytes: header_data });
                }
            }
        }
        Ok(Step::HeaderFrame)
    }

    // fn set_header(&mut self) {
    //     if let State::Header { channel_data, .. } = &self.state {
    //         let buf = ChunkedBuf::new(channel_data).reader();
    //         let channels = read_header(buf).expect("read header");
    //         println!("{:?}", channels);
    //         self.state = State::Data { channels };
    //     }
    // }

    fn data<'d>(&mut self, info: OsfPacketInfo, packet: &'d [u8]) -> Result<&'d [u8], Error> {
        if info.timestamp != self.header_timestamp {
            return Err(Error::StaleData {
                expected: self.header_timestamp,
                actual: info.timestamp,
            });
        }
        if let State::Data { .. } = &self.state {
            Ok(packet)
        } else {
            {
                println!("received data but still parsing header");
                Err(Error::EarlyData)
            }
        }
    }

    pub fn feed<'data>(&mut self, mut packet: &'data [u8]) -> Result<Step<'data>, Error> {
        let packet_info = OsfPacketInfo::read(&mut packet).unwrap();
        // dbg!(&packet_info);
        match packet_info.id {
            OsfPacketId::HeaderHeader => {
                let packet_count = self.header_header(packet_info, packet)?;
                Ok(Step::HeaderStart(packet_count))
            }
            OsfPacketId::HeaderData => {
                let step = self.header_data(packet_info, packet)?;
                Ok(step)
            }
            OsfPacketId::Data => {
                let blocks = self.data(packet_info, packet)?;
                Ok(Step::Blocks(blocks))
            }
        }
    }
}

struct ChunkedBuf<'a> {
    offset: usize,
    local_offset: usize,
    channel_data: &'a [Box<[u8]>],
}

impl<'a> ChunkedBuf<'a> {
    #[allow(unused)]
    fn new(channel_data: &'a [Box<[u8]>]) -> Self {
        Self {
            offset: 0,
            local_offset: 0,
            channel_data,
        }
    }
}

impl Buf for ChunkedBuf<'_> {
    fn remaining(&self) -> usize {
        // empty
        if self.offset == self.channel_data.len() {
            return 0;
        }
        // sum all remaining chunks
        let mut remaining = 0;
        for buf in &self.channel_data[self.offset..] {
            remaining += buf.len();
        }
        // subtract current chunk offset
        remaining - self.local_offset
    }

    fn chunk(&self) -> &[u8] {
        if self.offset == self.channel_data.len() {
            &[]
        } else {
            &self.channel_data[self.offset][self.local_offset..]
        }
    }

    fn advance(&mut self, mut cnt: usize) {
        // Each iteration advanced through one chunk
        while cnt > 0 {
            // Nothing left
            if self.offset == self.channel_data.len() {
                return;
            }
            // How much is left in current chunk?
            let local_remaining = self.channel_data[self.offset][self.local_offset..].as_ref().remaining();
            if local_remaining > cnt {
                // Chunk has enough bytes, finish
                // never goes out of bounds because of the condition
                self.local_offset += cnt;
                break;
            } else {
                // Chunk is too small, advance to next chunk
                cnt -= local_remaining;
                self.local_offset = 0;
                self.offset += 1;
            }
        }
    }
}

pub struct OsfBlockDecoder<'a, 'b> {
    pub packet: &'a [u8],
    pub channels: &'b [Channel],
}

impl<'a, 'b> OsfBlockDecoder<'a, 'b> {
    pub fn new(packet: &'a [u8], channels: &'b [Channel]) -> Self {
        Self { packet, channels }
    }
}

impl OsfBlockDecoder<'_, '_> {
    fn decode_block(&mut self) -> Result<Option<OsfBlock>, Error> {
        if self.packet.is_empty() {
            return Ok(None);
        } else if self.packet.starts_with(b"OCEAN_STREAM_FORMAT4") {
            return Err(Error::UnexpectedHeader);
        } else if self.packet.len() < OsfBlockCommon::encoded_size() {
            return Err(Error::NotEnoughForCommon);
        };

        let common = OsfBlockCommon::read(self.packet);
        // dbg!(&common, OsfBlockCommon::encoded_size());
        self.packet.advance(OsfBlockCommon::encoded_size());

        if (self.packet.len() as u64) < common.data_size() {
            return Err(Error::NotEnoughForData {
                actual: self.packet.len() as u32,
                needed: common.data_size() as u32,
            });
        }

        let block = OsfBlock::read(common, self.channels, &mut self.packet)?;
        Ok(Some(block))
    }
}

impl Iterator for OsfBlockDecoder<'_, '_> {
    type Item = Result<OsfBlock, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        self.decode_block().transpose()
    }
}

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Stale packet")]
    StaleData { expected: u64, actual: u64 },
    #[error("Stale packet")]
    HeaderRange { found: usize },
    #[error("Stale packet")]
    StaleHeader { expected: u64, actual: u64 },
    #[error("Early data packet")]
    EarlyData,
    #[error("Not enough bytes for common struct")]
    NotEnoughForCommon,
    #[error("Not enough bytes for block data")]
    NotEnoughForData { actual: u32, needed: u32 },
    #[error("Cannot read block {source}")]
    Block {
        #[from]
        source: block::Error,
    },
    #[error("Unexpected Header in Stream")]
    UnexpectedHeader,
}
