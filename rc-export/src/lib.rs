pub mod v2;

pub struct ClientConfig {
    id: u64,
    keyfile: String,
    listen: Option<String>,
}

impl ClientConfig {
    pub fn new(id: u64, keyfile: String, listen: impl Into<Option<String>>) -> Self {
        Self {
            id,
            keyfile,
            listen: listen.into(),
        }
    }
}
