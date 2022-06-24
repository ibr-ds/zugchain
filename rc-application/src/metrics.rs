#[cfg(feature = "metrics")]
lazy_static::lazy_static! {
    pub static ref BLOCK_NUMBER: prometheus::IntGauge = prometheus::register_int_gauge!(
        "raily_blockchain_block_num",
        "current head block of the blockchain"
    )
    .unwrap();
}

pub fn record_block_num(#[allow(unused)] num: u64) {
    #[cfg(feature = "metrics")]
    {
        BLOCK_NUMBER.set(num as i64)
    }
}
