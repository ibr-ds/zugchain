use std::{io::Cursor, path::PathBuf};

use argh::FromArgs;
use osf::header::read_header;
use prost::Message;
use rc_blockchain::{
    export::{transaction::TxType, Transaction},
    storage::{Files, Storage},
};

use rc_victoria::{write_blocks_to_victoria, VicotriaOpts};

#[derive(FromArgs)]
/// Write Blockchain to Victoria Metrics
struct Opts {
    /// metrics Job
    #[argh(option)]
    job: String,
    /// metrics Instance
    #[argh(option)]
    instance: String,
    /// victoria Metrics Url
    #[argh(option)]
    victoria: String,
    /// the OSF Channels XML File
    #[argh(option)]
    channel_file: PathBuf,
    /// blockchain directory
    #[argh(option)]
    blockchain: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let opts: Opts = argh::from_env();

    let files = Files::new(opts.blockchain, rc_blockchain::storage::StorageMode::Read);

    let base = files.base().header();
    let head = files.head();

    println!("head = {}, base = {}", head.number, base.number);

    let mut last_header = Vec::new();

    let mut blocks = Vec::new();
    for i in base.number + 1..head.number {
        let block = files
            .read_block(i)
            .ok_or_else(|| anyhow::anyhow!("block {} not found", i))?;
        let data = block.data.as_ref().unwrap();
        for tx in &data.data {
            let tx = Transaction::decode(tx.clone()).unwrap();
            let t = TxType::from_i32(tx.tx_type).unwrap();
            if t == TxType::Config {
                let header = read_header(Cursor::new(&tx.payload)).unwrap();
                if header != last_header {
                    println!("{:?}: {} channels", t, header.len());
                    last_header = header;
                    // println!("{:#?}", last_header);
                }
            }
        }
        blocks.push(block);
    }

    let vopts = VicotriaOpts {
        job: &opts.job,
        instance: &opts.instance,
        victoria: opts.victoria,
        channel_file: opts.channel_file,
    };

    write_blocks_to_victoria(vopts, blocks.iter()).await.unwrap();

    Ok(())
}
