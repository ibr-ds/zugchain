use anyhow::anyhow;
use prost::Message;
use rc_blockchain::{
    chainproto::{Block, Envelope},
    storage,
};
use std::{fs::File, io::stdout, path::PathBuf};
use storage::Storage;

use std::io::Write;

pub fn read_chain(directory: PathBuf) -> anyhow::Result<Vec<Block>> {
    let storage = storage::Files::new(directory, storage::StorageMode::Read);

    let mut blocks = Vec::new();
    for i in 0.. {
        match storage.read_block(i) {
            None => {
                println!("block {} not found", i);
                break;
            }
            Some(block) => blocks.push(block),
        }
    }

    Ok(blocks)
}

pub fn print_block(block: Block, out: &mut dyn Write) -> anyhow::Result<()> {
    let header = block.header.unwrap();
    let envelopes = block.data.unwrap().data;
    let envelopes = envelopes
        .into_iter()
        .map(|b| Envelope::decode(b.as_ref()))
        .collect::<Result<Vec<_>, _>>()?;

    let header_hash = base64::encode(&header.previous_hash);
    let data_hash = base64::encode(&header.data_hash);
    write!(out, "{:4},{},{}", header.number, header_hash, data_hash).unwrap();

    for env in envelopes {
        let lossy = String::from_utf8_lossy(&env.payload);
        write!(out, ", {}", lossy).unwrap();
    }

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args();
    let dir = args.nth(1).ok_or_else(|| anyhow!("expected one arg: directory"))?;
    let file = args.next();

    let mut write: Box<dyn Write> = if let Some(file) = file {
        Box::new(File::create(file).expect("create file"))
    } else {
        Box::new(stdout())
    };

    let blocks = read_chain(dir.into())?;
    for block in blocks {
        print_block(block, &mut write)?;
        writeln!(write).unwrap();
    }
    Ok(())
}
