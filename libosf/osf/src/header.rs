use std::{
    convert::Infallible,
    fmt::Display,
    io::{BufRead, Read},
    str::FromStr,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OsfDataType {
    Bool,
    Float,
    Double,
    Int64,
    Int32,
    Int16,
    Int8,
    String,
    GpsLocation,
    CanData,
    Custom(Box<str>),
}

impl Display for OsfDataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            OsfDataType::Bool => "bool",
            OsfDataType::Float => "double",
            OsfDataType::Double => "float",
            OsfDataType::Int64 => "int64",
            OsfDataType::Int32 => "int32",
            OsfDataType::Int16 => "int16",
            OsfDataType::Int8 => "int8",
            OsfDataType::String => "string",
            OsfDataType::GpsLocation => "gpslocation",
            OsfDataType::CanData => "candata",
            OsfDataType::Custom(s) => s,
        };
        write!(f, "{}", s)
    }
}

impl FromStr for OsfDataType {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = match s {
            "bool" => OsfDataType::Bool,
            "double" => OsfDataType::Double,
            "float" => OsfDataType::Float,
            "int64" => OsfDataType::Int64,
            "int32" => OsfDataType::Int32,
            "int16" => OsfDataType::Int16,
            "int8" => OsfDataType::Int8,
            "string" => OsfDataType::String,
            "gpslocation" => OsfDataType::GpsLocation,
            "candata" => OsfDataType::CanData,
            s => OsfDataType::Custom(s.into()), //TODO: validate struct custom type syntax
        };
        Ok(t)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OsfChannelType {
    Scalar,
}

impl Display for OsfChannelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "scalar")
    }
}

impl FromStr for OsfChannelType {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "scalar" => Ok(OsfChannelType::Scalar),
            _ => Err(Error::UnknownChannelType { channel_type: s.into() }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Channel {
    pub index: usize,
    pub name: String,
    #[serde(with = "serde_with::rust::display_fromstr")]
    pub datatype: OsfDataType,
    #[serde(with = "serde_with::rust::display_fromstr")]
    pub channeltype: OsfChannelType,
    pub sizeoflengthvalue: usize,
}

#[derive(Debug, Deserialize)]
struct Channels {
    #[serde(rename = "channel", default)]
    channels: Vec<Channel>,
}

#[derive(Debug, Deserialize)]
struct Optimeas {
    channels: Channels,
}

pub fn read_header<R: BufRead>(mut stream: R) -> Result<Vec<Channel>, Error> {
    let mut string_buf = String::with_capacity(30);

    stream.read_line(&mut string_buf).map_err(|_| Error::MagicHeader)?;
    let xml_length = magic_header(&string_buf)?;

    string_buf.clear();
    stream.take(xml_length).read_to_string(&mut string_buf)?;

    parse_xml(&string_buf)
}

#[cfg(feature = "tokio")]
pub async fn read_header_async<R: tokio::io::AsyncBufRead + Unpin>(mut stream: R) -> Result<Vec<Channel>, Error> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};

    let mut string_buf = String::with_capacity(30);

    stream
        .read_line(&mut string_buf)
        .await
        .map_err(|_| Error::MagicHeader)?;
    let xml_length = magic_header(&string_buf)?;

    string_buf.clear();
    stream.take(xml_length).read_to_string(&mut string_buf).await?;

    parse_xml(&string_buf)
}

pub fn parse_xml(string_buf: &str) -> Result<Vec<Channel>, Error> {
    let optimeas: Optimeas = quick_xml::de::from_str(string_buf)?;
    Ok(optimeas.channels.channels)
}

fn magic_header(string_buf: &str) -> Result<u64, Error> {
    let mut splits = string_buf.split_ascii_whitespace();
    match splits.next() {
        Some("OCEAN_STREAM_FORMAT4") => {}
        _ => return Err(Error::MagicHeader),
    };
    let xml_length: u64 = splits
        .next()
        .ok_or(Error::MagicHeaderLength)?
        .parse()
        .map_err(|_| Error::MagicHeaderLength)?;
    Ok(xml_length)
}

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid magic header")]
    MagicHeader,
    #[error("invalid magic header length")]
    MagicHeaderLength,
    #[error("xml: {source}")]
    Xml {
        #[from]
        source: quick_xml::DeError,
    },
    #[error("io {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },
    #[error("unknown channel type: {channel_type}")]
    UnknownChannelType { channel_type: Box<str> },
}
