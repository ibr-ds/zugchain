pub mod storage;

mod proto;
pub use proto::{common as chainproto, export, mvbproto};

macro_rules! none_return {
    ($value:expr,$ret:literal) => {
        match $value {
            Some(v) => v,
            None => return $ret,
        }
    };
}
pub mod hashing;

pub mod genesis;

pub mod display;

pub fn to_vec<M: prost::Message>(message: &M) -> Result<Vec<u8>, prost::EncodeError> {
    let mut buf = Vec::with_capacity(message.encoded_len());
    message.encode(&mut buf)?;
    Ok(buf)
}
