use std::fmt::Debug;

use rc_blockchain::{chainproto::BlockHeader, hashing::hash_header};
use ring::signature::{Ed25519KeyPair, UnparsedPublicKey};
use ring_compat::digest::Sha256;

pub struct Keys {
    signer: Ed25519KeyPair,
    cloud: UnparsedPublicKey<Vec<u8>>,
}

impl Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secret Key")
    }
}

impl Keys {
    pub fn new(signer: Ed25519KeyPair, cloud: UnparsedPublicKey<Vec<u8>>) -> Self {
        Self { signer, cloud }
    }

    pub fn verify_header(&self, header: &BlockHeader, signature: &[u8]) -> bool {
        let digest = hash_header::<Sha256>(header);
        self.cloud.verify(&digest, signature).is_ok()
    }

    pub fn sign_header(&self, header: &BlockHeader) -> Vec<u8> {
        let digest = hash_header::<Sha256>(header);
        Vec::from(self.signer.sign(&digest).as_ref())
    }
}
