use std::{
    convert::TryFrom,
    fmt::Display,
    io::Read,
    mem::size_of,
    str::{from_utf8, Utf8Error},
};

use bytes::Buf;

use crate::header::{Channel, OsfDataType};

#[derive(Debug, serde::Serialize)]
pub struct GpsLocation {
    latitude: f64,
    longitude: f64,
    altitude: f64,
}

#[derive(Debug, serde::Serialize)]
pub enum OsfValue {
    Bool(bool),
    Float(f32),
    Double(f64),
    Int64(i64),
    Int32(i32),
    Int16(i16),
    Int8(i8),
    GpsLocation(Box<GpsLocation>),
}

impl TryFrom<&OsfValue> for f64 {
    type Error = ();

    fn try_from(value: &OsfValue) -> Result<Self, Self::Error> {
        match *value {
            OsfValue::Bool(value) => Ok(u8::from(value).into()),
            OsfValue::Float(value) => Ok(value.into()),
            OsfValue::Double(value) => Ok(value),
            OsfValue::Int64(value) => Ok(value as f64),
            OsfValue::Int32(value) => Ok(value.into()),
            OsfValue::Int16(value) => Ok(value.into()),
            OsfValue::Int8(value) => Ok(value.into()),
            _ => Err(()),
        }
    }
}

// impl OsfValue {
//     fn encoded_size(&self) -> usize {
//         match self {
//             OsfValue::Bool(_) => size_of::<bool>(),
//             OsfValue::Float(_) => size_of::<f32>(),
//             OsfValue::Double(_) => size_of::<f64>(),
//             OsfValue::Int64(_) => size_of::<i64>(),
//             OsfValue::Int32(_) => size_of::<i32>(),
//             OsfValue::Int16(_) => size_of::<i16>(),
//             OsfValue::Int8(_) => size_of::<i8>(),
//             OsfValue::GpsLocation {..} => size_of::<Box<GpsLocation>>(),
//         }
//     }
// }

#[derive(Debug, serde::Serialize)]
pub struct OsfBlock {
    pub common: OsfBlockCommon,
    pub data: OsfBlockSample,
}

impl OsfBlock {
    pub fn read<B: Buf>(common: OsfBlockCommon, channels: &[Channel], mut buffer: B) -> Result<OsfBlock, Error> {
        let channel = channels
            .get(common.channel_index as usize)
            .ok_or(Error::ChannelNotFound)?;

        let data = match common.control_character {
            GENERIC_SINGLE_VALUE => {
                let value = read_value(channel, buffer)?;
                OsfBlockSample::GenericSingleValue(value)
            }
            GENERIC_MULTI_VALUE => {
                let value = read_multi_value(channel, buffer)?;
                OsfBlockSample::GenericMultiValue(value)
            }
            STRING => {
                let value = read_string(buffer.chunk())?;
                buffer.advance(value.value.len());
                OsfBlockSample::String(value)
            }
            _ => return Err(Error::UnknownType),
        };

        Ok(OsfBlock { common, data })
    }

    pub fn display<'c, 's>(&'s self, channels: &'c [Channel]) -> BlockDisplay<'c, 's> {
        let width = channels.iter().map(|c| c.name.len()).max().unwrap_or_default();
        BlockDisplay {
            width,
            channel: &channels[self.common.channel_index as usize],
            block: self,
        }
    }
}

#[derive(Debug)]
pub struct BlockDisplay<'c, 's> {
    width: usize,
    channel: &'c Channel,
    block: &'s OsfBlock,
}

impl<'c, 's> Display for BlockDisplay<'c, 's> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.block.data {
            OsfBlockSample::GenericSingleValue(value) => {
                let ts = parse_timestamp(value.timestamp);
                writeln!(f, "{{")?;
                writeln!(
                    f,
                    "  {:<width$} | {} | {:?}",
                    &self.channel.name,
                    ts,
                    value.value,
                    width = self.width
                )?;
                write!(f, "}}")?;
                Ok(())
            }
            OsfBlockSample::GenericMultiValue(values) => {
                writeln!(f, "{{")?;
                for value in values.iter() {
                    let ts = parse_timestamp(value.timestamp);

                    writeln!(
                        f,
                        "  {:<width$} | {} | {:?}",
                        &self.channel.name,
                        ts,
                        value.value,
                        width = self.width
                    )?;
                }
                write!(f, "}}")?;
                Ok(())
            }
            OsfBlockSample::String(value) => {
                let ts = parse_timestamp(value.timestamp);

                writeln!(f, "{{")?;
                writeln!(
                    f,
                    "  {:<width$} | {} | {:?}",
                    &self.channel.name,
                    ts,
                    value.value,
                    width = self.width
                )?;
                write!(f, "}}")?;
                Ok(())
            }
        }
    }
}

fn parse_timestamp(value: i64) -> chrono::DateTime<chrono::FixedOffset> {
    use chrono::TimeZone;
    chrono::FixedOffset::east(2 * 3600).timestamp(value / 1_000_000_000, (value % 1_000_000_000) as u32)
}

#[derive(Debug, serde::Serialize)]
pub struct OsfBlockCommon {
    pub channel_index: u16,
    block_length: u32,
    pub control_character: u8,
}

impl OsfBlockCommon {
    pub fn data_size(&self) -> u64 {
        self.block_length.saturating_sub(1) as u64
    }

    pub const fn encoded_size() -> usize {
        size_of::<u16>() + size_of::<u32>() + size_of::<u8>()
    }

    pub fn read<B: Buf>(mut buf: B) -> Self {
        let channel_index = buf.get_u16_le();
        let block_length = buf.get_u32_le();
        let control_character = buf.get_u8();
        Self {
            channel_index,
            block_length,
            control_character,
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct OsfSample {
    pub timestamp: i64,
    pub value: OsfValue,
}

#[derive(Debug, serde::Serialize)]
pub enum OsfBlockSample {
    GenericSingleValue(OsfSample),
    GenericMultiValue(Box<[OsfSample]>),
    String(StringValue),
}

#[derive(Debug)]
pub struct OsfStream<R> {
    decode_buf: Vec<u8>,
    channels: Vec<Channel>,
    stream: R,
}

impl<R> OsfStream<R>
where
    R: Read,
{
    pub fn next_block(&mut self) -> Result<OsfBlock, Error> {
        let mut header = [0; OsfBlockCommon::encoded_size()];
        self.stream
            .read_exact(&mut header)
            .map_err(|source| Error::BlockCommon { source })?;
        let common = OsfBlockCommon::read(header.as_ref());

        self.decode_buf.clear();
        self.stream
            .by_ref()
            .take(common.data_size())
            .read_to_end(&mut self.decode_buf)?;

        self.decode_block(common)
    }
}

impl<R> OsfStream<R> {
    pub fn new(channels: Vec<Channel>, stream: R) -> Self {
        Self {
            decode_buf: Vec::new(),
            channels,
            stream,
        }
    }

    #[cfg(feature = "tokio")]
    pub fn make_async(self) -> AsyncOsfStream<R> {
        AsyncOsfStream::new(self)
    }

    pub fn channels(&self) -> &[Channel] {
        &self.channels
    }

    pub fn channel(&self, index: u16) -> Option<&Channel> {
        self.channels.get(index as usize)
    }

    fn decode_block(&mut self, common: OsfBlockCommon) -> Result<OsfBlock, Error> {
        let channel = self
            .channels
            .get(common.channel_index as usize)
            .ok_or(Error::ChannelNotFound)?;

        let data = match common.control_character {
            GENERIC_SINGLE_VALUE => {
                let value = read_value(channel, self.decode_buf.as_slice())?;
                OsfBlockSample::GenericSingleValue(value)
            }
            GENERIC_MULTI_VALUE => {
                let value = read_multi_value(channel, self.decode_buf.as_ref())?;
                OsfBlockSample::GenericMultiValue(value)
            }
            STRING => {
                let value = read_string(self.decode_buf.as_ref())?;
                OsfBlockSample::String(value)
            }
            _ => return Err(Error::UnknownType),
        };

        Ok(OsfBlock { common, data })
    }
}

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Could not read common")]
    BlockCommon { source: std::io::Error },
    #[error("io: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },
    #[error("osf type unknown")]
    UnknownType,
    #[error("osf type {:?} not supported", 0)]
    UnsuportedType(OsfDataType),
    #[error("osf chanenl not found")]
    ChannelNotFound,
    #[error("string: {source}")]
    String {
        #[from]
        source: Utf8Error,
    },
}

#[derive(Debug, serde::Serialize)]
pub struct StringValue {
    pub timestamp: i64,
    pub value: Box<str>,
}

fn read_string(mut block: &[u8]) -> Result<StringValue, Error> {
    let timestamp = block.get_i64_le();
    let _len = block.get_u32_le();
    let s = from_utf8(block)?;
    Ok(StringValue {
        timestamp,
        value: s.into(),
    })
}

fn read_value<B: Buf>(channel: &Channel, mut block: B) -> Result<OsfSample, Error> {
    let timestamp = block.get_i64_le();

    let value = match &channel.datatype {
        OsfDataType::Bool => {
            let b = block.get_u8() > 0;
            OsfValue::Bool(b)
        }
        OsfDataType::Double => OsfValue::Double(block.get_f64_le()),
        OsfDataType::Float => OsfValue::Float(block.get_f32_le()),
        OsfDataType::Int64 => OsfValue::Int64(block.get_i64_le()),
        OsfDataType::Int32 => OsfValue::Int32(block.get_i32_le()),
        OsfDataType::Int16 => OsfValue::Int16(block.get_i16_le()),
        OsfDataType::Int8 => OsfValue::Int8(block.get_i8()),
        OsfDataType::GpsLocation => OsfValue::GpsLocation(Box::new(GpsLocation {
            latitude: block.get_f64_le(),
            longitude: block.get_f64_le(),
            altitude: block.get_f64_le(),
        })),
        t => return Err(Error::UnsuportedType(t.clone())),
    };
    Ok(OsfSample { timestamp, value })
}

fn read_multi_value<B: Buf>(channel: &Channel, mut block: B) -> Result<Box<[OsfSample]>, Error> {
    let count = block.get_u32_le();

    let mut buf = Vec::with_capacity(count as usize);
    for _i in 0..count {
        let value = read_value(channel, &mut block)?;
        buf.push(value)
    }
    Ok(buf.into_boxed_slice())
}

const GENERIC_SINGLE_VALUE: u8 = 8;
const GENERIC_MULTI_VALUE: u8 = 8 | 128;
const STRING: u8 = 4;

#[cfg(feature = "tokio")]
mod async_stream {
    use tokio::io::AsyncReadExt;

    use super::*;

    pub struct AsyncOsfStream<R> {
        inner: OsfStream<R>,
    }

    impl<R> AsyncOsfStream<R> {
        pub fn new(inner: OsfStream<R>) -> Self {
            Self { inner }
        }
    }

    #[cfg(feature = "tokio")]
    impl<R> AsyncOsfStream<R>
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        pub async fn next_block(&mut self) -> Result<OsfBlock, Error> {
            let mut header = [0; OsfBlockCommon::encoded_size()];
            self.inner
                .stream
                .read_exact(&mut header)
                .await
                .map_err(|source| Error::BlockCommon { source })?;
            let common = OsfBlockCommon::read(header.as_ref());

            self.inner.decode_buf.clear();
            (&mut self.inner.stream)
                .take(common.data_size())
                .read_to_end(&mut self.inner.decode_buf)
                .await?;

            self.inner.decode_block(common)
        }
    }
}
#[cfg(feature = "tokio")]
pub use async_stream::*;
