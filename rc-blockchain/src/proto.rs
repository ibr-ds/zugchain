macro_rules! proto_include {
    ($path:tt) => {
        include!(concat!(env!("OUT_DIR"), "/", stringify!($path), ".rs"));
    };
}

macro_rules! protomod {
    ($path:tt) => {
        pub mod $path {
            include!(concat!(env!("OUT_DIR"), "/", stringify!($path), ".rs"));
        }
    };
    ($path:tt, $($stuff:item)+) => {
        pub mod $path {
            include!(concat!(env!("OUT_DIR"), "/", stringify!($path), ".rs"));
            $($stuff)+
        }
    }
}

pub mod common {
    proto_include!(common);

    use crate::display::DisplayBytes;
    use std::fmt::{self, Display};

    impl Block {
        pub fn number(&self) -> u64 {
            self.header.as_ref().unwrap().number
        }

        pub fn header(&self) -> &BlockHeader {
            self.header.as_ref().unwrap()
        }
    }

    impl Envelope {
        pub fn new(payload: bytes::Bytes) -> Self {
            Envelope {
                payload,
                signature: Default::default(),
            }
        }
    }
    impl Display for BlockHeader {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                f,
                "{{ {}, {}, {} }}",
                self.number,
                DisplayBytes(&self.previous_hash),
                DisplayBytes(&self.data_hash)
            )
        }
    }
}
protomod!(mvbproto);
pub mod storage {
    proto_include!(storage);

    pub type Base = super::export::Delete;
}

#[allow(clippy::module_inception)]
pub mod export {
    use super::common::BlockHeader;
    use bytes::Bytes;

    proto_include!(export);

    impl Delete {
        pub fn number(&self) -> u64 {
            self.header.as_ref().unwrap().number
        }

        pub fn header(&self) -> &super::common::BlockHeader {
            self.header.as_ref().unwrap()
        }
    }

    impl Command {
        pub fn transaction(payload: Bytes) -> Self {
            Self {
                action: Some(command::Action::Transaction(Transaction {
                    payload,
                    tx_type: transaction::TxType::Unspecified.into(),
                })),
            }
        }

        pub fn transaction2(tx: Transaction) -> Self {
            Self {
                action: Some(command::Action::Transaction(tx)),
            }
        }

        pub fn read(base: u64) -> Self {
            let read = Read {
                base,
                head: 0,
                version: ReadOp::V1.into(),
            };
            let read = export::Command::Read(read);
            let read = Export { command: Some(read) };

            Self {
                action: Some(command::Action::Export(read)),
            }
        }

        pub fn read2(base: u64) -> Self {
            let read = Read {
                base,
                head: 0,
                version: ReadOp::V2.into(),
            };
            let read = export::Command::Read(read);
            let read = Export { command: Some(read) };

            Self {
                action: Some(command::Action::Export(read)),
            }
        }

        pub fn read0(base: u64, head: u64) -> Self {
            let read = Read {
                base,
                head,
                version: ReadOp::Read.into(),
            };
            let read = export::Command::Read(read);
            let read = Export { command: Some(read) };

            Self {
                action: Some(command::Action::Export(read)),
            }
        }

        pub fn delete(header: BlockHeader, signature: Vec<u8>) -> Self {
            let delete = Delete {
                header: Some(header),
                signature,
            };
            let cmd = export::Command::Delete(delete);
            let cmd = Export { command: Some(cmd) };

            Self {
                action: Some(command::Action::Export(cmd)),
            }
        }
    }
}
