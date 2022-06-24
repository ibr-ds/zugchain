#![feature(type_alias_impl_trait)]

use std::time::{SystemTime, UNIX_EPOCH};

pub mod app;
// #[cfg(feature = "mvb")]
// pub mod mvb;

#[cfg(feature = "osf")]
pub mod osf;

pub mod metrics;

// async fn blockchain(
//     mut receiver: Receiver<Message<Raw<Client>>>,
//     mut app: Railchain<impl Storage>,
// ) -> anyhow::Result<()> {
//     while let Some(message) = receiver.next().await {
//         let message = message.unpack()?;
//         app.execute(message).await?;
//     }

//     Ok(())
// }

pub use rc_blockchain::to_vec;
pub mod request_generator;

pub fn timestamp() -> u64 {
    let now = SystemTime::now();
    let epoch = now.duration_since(UNIX_EPOCH).unwrap();
    epoch.as_nanos() as u64
}

#[derive(PartialEq, Eq)]
pub enum Fm {
    One,
    Two { me: u64 },
}
