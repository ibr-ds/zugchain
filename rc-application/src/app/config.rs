use std::{fs, path::PathBuf};

use ring::signature::{Ed25519KeyPair, UnparsedPublicKey, ED25519};
use serde::{Deserialize, Serialize};

use super::crypto::Keys;

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub max_block_messages: usize,
    // pub preferred_block_size: usize,
    // pub max_block_size: usize,
    pub export: Export,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Export {
    #[serde(default)]
    pub signing_key: PathBuf,
    pub exporter_identity: PathBuf,
}

impl Config {
    pub fn load_keys(&self, me: u64) -> anyhow::Result<Keys> {
        let private_bytes = if self.export.signing_key != PathBuf::default() {
            fs::read(&self.export.signing_key)?
        } else {
            let file = format!("keys/ed-25519-private-{}", me);
            fs::read(file)?
        };

        let private_key = Ed25519KeyPair::from_pkcs8(&private_bytes)?;

        let public_bytes = fs::read(&self.export.exporter_identity)?;
        let public_key = UnparsedPublicKey::new(&ED25519, public_bytes);

        Ok(Keys::new(private_key, public_key))
    }
}
