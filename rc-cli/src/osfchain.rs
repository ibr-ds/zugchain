use std::io::{Cursor, Write};

use prost::Message;
use rc_blockchain::{
    chainproto::Block,
    export::{transaction::TxType, Transaction},
};

#[allow(unused)]
pub fn blocks_to_osf(blocks: &[Block], file: &mut dyn Write) {
    let transactions = blocks
        .iter()
        .flat_map(|b| b.data.as_ref().unwrap().data.clone())
        .map(|tx| Transaction::decode(Cursor::new(tx)).unwrap());

    let transactions =
        transactions.skip_while(|tx| tx.tx_type == TxType::Data as i32 || tx.tx_type == TxType::Unspecified as i32);

    for tx in transactions {
        file.write_all(&tx.payload).unwrap()
    }
}
