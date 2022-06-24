use std::{
    borrow::Cow,
    fs::File,
    io::{BufRead, Write},
    path::PathBuf,
};

use osf::{
    block::{OsfBlock, OsfValue},
    datagram::OsfBlockDecoder,
    header::{read_header, Channel},
};
use prost::{bytes::Buf, Message};
use rc_blockchain::{
    chainproto::Block,
    export::{transaction::TxType, Transaction},
};
use reqwest::Body;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct Metric<'s> {
    #[serde(rename = "__name__")]
    name: Cow<'s, str>,
    job: Cow<'s, str>,
    instance: Cow<'s, str>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum Value {
    Int(i64),
    Float(f64),
    String(Box<str>),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VictoriaLine<'s> {
    metric: Metric<'s>,
    values: Vec<Value>,
    timestamps: Vec<i64>,
}

fn block_to_victoria<'s>(
    job: &'s str,
    instance: &'s str,
    channels: &'s [Channel],
    block: &'s OsfBlock,
) -> Option<VictoriaLine<'s>> {
    let channel = &channels[block.common.channel_index as usize];

    let metric = Metric {
        name: channel.name.as_str().into(),
        job: job.into(),
        instance: instance.into(),
    };

    match &block.data {
        osf::block::OsfBlockSample::GenericSingleValue(sample) => {
            let value = osf_value_to_victoria(sample)?;
            Some(VictoriaLine {
                metric,
                values: vec![value],
                timestamps: vec![sample.timestamp / 1_000_000],
            })
        }
        osf::block::OsfBlockSample::GenericMultiValue(multi) => {
            let values: Vec<Value> = multi.iter().map(osf_value_to_victoria).collect::<Option<_>>()?;
            let timestamps: Vec<_> = multi.iter().map(|s| s.timestamp / 1_000_000).collect();
            Some(VictoriaLine {
                metric,
                values,
                timestamps,
            })
        }
        osf::block::OsfBlockSample::String(string) => Some(VictoriaLine {
            metric,
            values: vec![Value::String(string.value.clone())],
            timestamps: vec![string.timestamp / 1_000_000],
        }),
    }
}

fn osf_value_to_victoria(sample: &osf::block::OsfSample) -> Option<Value> {
    let value = match sample.value {
        OsfValue::Bool(v) => Value::Int(v.into()),
        OsfValue::Float(v) => Value::Float(v.into()),
        OsfValue::Double(v) => Value::Float(v),
        OsfValue::Int64(v) => Value::Int(v),
        OsfValue::Int32(v) => Value::Int(v.into()),
        OsfValue::Int16(v) => Value::Int(v.into()),
        OsfValue::Int8(v) => Value::Int(v.into()),
        OsfValue::GpsLocation(_) => {
            tracing::warn!("GPS Location not supported by victoria");
            return None;
        }
    };
    Some(value)
}

pub struct VicotriaOpts<'s> {
    pub job: &'s str,
    pub instance: &'s str,
    pub victoria: String,
    pub channel_file: PathBuf,
}

pub async fn write_blocks_to_victoria(
    mut opts: VicotriaOpts<'_>,
    blocks: impl Iterator<Item = &Block>,
) -> anyhow::Result<()> {
    const IMPORT_API: &str = "/api/v1/import";
    opts.victoria.push_str(IMPORT_API);

    let file = std::fs::read_to_string(&opts.channel_file)?;
    let mut channels = osf::header::parse_xml(&file)?;

    let client = reqwest::Client::new();

    let mut metrics = Vec::with_capacity(4096);
    for block in blocks {
        let data = block.data.as_ref().unwrap();
        tracing::debug!("block {}", block.number());
        for tx in &data.data {
            let tx = Transaction::decode(tx.as_ref()).unwrap();
            tracing::debug!("txtype {}", tx.tx_type);
            let config = tx.tx_type == TxType::Config as i32;

            if config {
                let new_channels = read_header(tx.payload.as_ref())?;
                for (i, (old_chan, new_chan)) in channels.iter().zip(&new_channels).enumerate() {
                    if old_chan != new_chan {
                        tracing::warn!(i, ?old_chan, ?new_chan, "Channel Config Changed. Adopting new config.");
                        channels = new_channels;

                        let mut file = std::io::BufWriter::new(File::create(&opts.channel_file).unwrap());
                        for line in tx.payload.reader().lines().skip(1) {
                            let line = line.unwrap();
                            file.write_all(line.as_ref()).unwrap();
                        }
                        file.flush().unwrap();

                        break;
                    }
                }
            } else {
                let blocks = OsfBlockDecoder::new(&tx.payload, &channels);
                for block in blocks {
                    tracing::debug!("osfblock");
                    let block = block.unwrap();
                    if let Some(line) = block_to_victoria(opts.job, opts.instance, &channels, &block) {
                        serde_json::to_writer(&mut metrics, &line).unwrap();
                        metrics.write_all(b"\n").unwrap();
                    }
                }
            }
        }
    }

    let request = client.post(opts.victoria).body(Body::from(metrics)).send().await?;
    if !request.status().is_success() {
        anyhow::bail!("{}", request.status())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::VictoriaLine;

    const TEST_LINES: &str = r#"{"metric":{"__name__":"up","job":"node_exporter","instance":"localhost:9100"},"values":[0,0,0],"timestamps":[1549891472010,1549891487724,1549891503438]}
    {"metric":{"__name__":"up","job":"prometheus","instance":"localhost:9090"},"values":[1,1,1],"timestamps":[1549891461511,1549891476511,1549891491511]}"#;

    #[test]
    fn deserialize() {
        let lines = TEST_LINES.lines();

        for line in lines {
            let _object: VictoriaLine = serde_json::from_str(line).unwrap();
        }
    }
}
