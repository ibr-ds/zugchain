use std::{env, path::PathBuf};

macro_rules! fabric {
    ($path:literal) => {
        concat!(fabric!(), $path)
    };
    () => {
        "../vendor/fabric-protos/"
    };
}

macro_rules! rc {
    ($path:literal) => {
        concat!(rc!(), $path)
    };
    () => {
        "proto/"
    };
}

macro_rules! derive {
    ($($t:path),*) => {
        stringify!(#[derive($($t),*)])
    };
}

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    const SERDE: &str = stringify!(#[derive(serde::Serialize, serde::Deserialize)]);
    const SERDE_BYTES: &str = stringify!(#[serde(with = "serde_bytes")]);

    println!("cargo:rerun-if-changed={}", fabric!("common/common.proto"));
    println!("cargo:rerun-if-changed={}", rc!("mvbsignal.proto"));
    println!("cargo:rerun-if-changed={}", rc!("export.proto"));
    println!("cargo:rerun-if-changed={}", rc!("storage.proto"));

    prost_build::Config::new()
        .out_dir(&out_dir)
        .type_attribute("Envelope", SERDE)
        .type_attribute("Block", SERDE)
        .type_attribute(
            "BlockHeader",
            derive!(PartialOrd, Ord, Eq, serde::Serialize, serde::Deserialize),
        )
        .type_attribute("BlockData", SERDE)
        .type_attribute("BlockMetadata", SERDE)
        .type_attribute("export.Delete", SERDE)
        .type_attribute("export.Transaction", SERDE)
        // .field_attribute("Envelope.payload", r#"#[serde(with = "serde_bytes")]"#)
        .field_attribute("Envelope.signature", SERDE_BYTES)
        .bytes(&[
            "Envelope.payload",
            "Command.transaction",
            "CheckpointedBlocks.blocks",
            "export.Transaction.payload",
            "BlockData.data",
        ])
        // .bytes(&["Envelope.payload"])
        .compile_protos(
            &[
                fabric!("common/common.proto"),
                rc!("mvbsignal.proto"),
                rc!("export.proto"),
                rc!("storage.proto"),
            ],
            &[fabric!("common/"), rc!()],
        )
        .unwrap();
}
