use std::{fmt::Debug, io::Read};

use anyhow::{Context, Result};
use bytes::{Bytes, BytesMut};
use rc_blockchain::{
    export::{command::Action, Command},
    to_vec,
};
use serde::de::DeserializeOwned;
use structopt::StructOpt;
use tokio_util::codec::{Decoder, Encoder};

use themis_core::{
    app::{Client, Request},
    authentication::dummy::Dummy,
    net::{Message, MessageCodec, RawMessage, Tag},
};

fn validate_message<T>(bytes: &[u8]) -> Result<T>
where
    T: DeserializeOwned + Debug,
{
    let message: T = rmp_serde::from_slice(bytes)?;

    println!("Decoded Message: ");
    println!("{:#?}\n", message);
    Ok(message)
}

fn validate_length_delim<T: Tag>(stream: &mut BytesMut) -> Result<RawMessage<T>> {
    let mut message_codec = MessageCodec::<T>::new(Box::new(Dummy));

    let decoded = message_codec.decode(stream)?.context("Incomplete Message")?;

    println!("Decoded Message: ");
    println!("{:#?}\n", decoded);

    Ok(decoded)
}

fn read(mut stream: impl Read) -> Result<BytesMut> {
    let mut vec = Vec::new();
    stream.read_to_end(&mut vec)?;
    Ok(BytesMut::from(vec.as_slice()))
}

fn demo(output_length: bool) -> Vec<u8> {
    println!(
        "Demo: 'Hello World' from client 222, sent to node 333, with timestamp 111, to be written to the blockchain"
    );
    println!("With fake signature 'signed'");

    let command = Command::transaction("Hello World".into());
    let command_vec = to_vec(&command).unwrap();

    println!("Generated Command: {:?}", command);
    println!("Serialized (protobuf): {:x?}", command_vec);

    let request = Request::new(111, command_vec.into());
    let request_bytes = rmp_serde::to_vec(&request).unwrap();

    println!("Request: {:?}", request);
    println!("Serialized (msgpack): {:x?}", request_bytes);

    let message = Message::new(222, 333, request);
    let mut message_packed = message.pack().unwrap();
    message_packed.signature = Bytes::from_static(b"signed");
    let bytes = rmp_serde::to_vec(&message_packed).unwrap();

    println!("Message: {:?}", message);
    println!("Serialized (msgpack): {:x?}", bytes);
    println!("Message length: {}", bytes.len());

    let mut buf = BytesMut::new();
    let mut message_codec = MessageCodec::<Client>::new(Box::new(Dummy));
    message_codec.encode(message_packed, &mut buf).unwrap();

    println!("With 4 byte length prefix: {:x?}", &buf[..]);

    if output_length {
        let mut vec = Vec::new();
        vec.extend(buf);
        vec
    } else {
        bytes
    }
}

#[allow(unused)]
fn print_analyize(bytes: &[u8]) {
    let base64 = base64::encode_config(&bytes, base64::STANDARD);
    println!(
        "Analyze serialized message: https://msgpack.dbrgn.ch/#base64={}",
        base64
    );
}

#[derive(Debug, StructOpt)]
enum Verbs {
    Validate {
        #[structopt(long, short = "l")]
        with_length: bool,
        #[structopt(default_value = "-")]
        file: String,
    },
    Demo {
        #[structopt(long, short = "l")]
        with_length: bool,
        #[structopt(long, default_value = "")]
        file: String,
    },
}

#[derive(Debug, StructOpt)]
struct Opts {
    #[structopt(subcommand)]
    verb: Verbs,
}

fn main() -> Result<()> {
    let opts = Opts::from_args();

    match opts.verb {
        Verbs::Demo { file, with_length } => {
            let bytes = demo(with_length);
            if !file.is_empty() {
                std::fs::write(&file, bytes).context(file)?;
            }
        }
        Verbs::Validate { file, with_length } => {
            let mut bytes = if file == "-" {
                let stdin = std::io::stdin();
                let stdin = stdin.lock();
                read(stdin)?
            } else {
                let file = std::fs::File::open(&file).context(file)?;
                read(file)?
            };

            println!("Input: ");
            println!("{:?}\n", bytes.as_ref());

            let request: Message<Request> = if with_length {
                let message = validate_length_delim::<Client>(&mut bytes)?;
                message.unpack()?
            } else {
                let message = validate_message::<RawMessage<Client>>(&bytes)?;
                message.unpack()?
            };

            println!("Request:");
            println!("{:#?}\n", request);

            use prost::Message as _;
            let command = Command::decode(request.inner.payload).unwrap();
            println!("Command: {:?}", command);

            if let Some(Action::Transaction(s)) = command.action {
                if let Ok(s) = std::str::from_utf8(&s.payload) {
                    println!("{}", s);
                }
            }
        }
    }

    Ok(())
}
