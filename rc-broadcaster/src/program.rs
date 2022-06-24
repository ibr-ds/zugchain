use anyhow::Context;
use bytes::Bytes;
use rc_blockchain::{export::Command, to_vec};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, time::Instant};
use themis_core::{app::Request, net::Message};

pub fn load_program(program: &Path) -> anyhow::Result<Vec<Kind>> {
    let file = fs::File::open(&program).context("open progam file")?;
    let mut reader = std::io::BufReader::new(file);
    let program: Vec<Kind> = ron::de::from_reader(&mut reader).context("parse program")?;

    Ok(program)
}

const fn default_n() -> u64 {
    1
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum Kind {
    Normal,
    Skip {
        to: u64,
    },
    Corrupt {
        value: String,
        to: u64,
    },
    Additional {
        to: u64,
        #[serde(default = "default_n")]
        n: u64,
    },
}

impl Kind {
    fn expected_fm2(&self) -> usize {
        match self {
            Kind::Normal => 1,
            Kind::Skip { .. } => 1,
            Kind::Corrupt { .. } => 2,
            Kind::Additional { n, .. } => *n as usize + 1,
        }
    }

    fn create_messages(&self, sequence: u64, payload: String, replicas: u64) -> Vec<Message<Request>> {
        match self {
            Kind::Normal => broadcast(0, sequence, replicas, &payload),
            Kind::Skip { to } => {
                let mut messages = Vec::with_capacity(replicas as usize);
                for i in 0..replicas {
                    if i == *to {
                        continue;
                    }
                    let message = message(0, i, sequence, &payload);
                    messages.push(message);
                }
                messages
            }
            Kind::Corrupt {
                value: corrupt_payload,
                to,
            } => {
                let mut messages = Vec::with_capacity(replicas as usize);
                for i in 0..replicas {
                    if i == *to {
                        let message = message(0, i, sequence, corrupt_payload);
                        messages.push(message);
                    } else {
                        let message = message(0, i, sequence, &payload);
                        messages.push(message);
                    }
                }
                messages
            }

            Kind::Additional { to, n } => {
                let mut msgs = Vec::with_capacity(1 + *n as usize);
                for j in 0..*n {
                    let payload = payload.clone() + &format!("extra{}", j);
                    let add = message(0, *to, sequence, &payload);
                    msgs.push(add);
                }
                let message = broadcast(0, sequence, replicas, &payload);
                msgs.extend(message);
                msgs
            }
        }
    }

    fn create_messages_fm2(&self, sequence: u64, payload: String, replicas: u64) -> Vec<Message<Request>> {
        match self {
            Kind::Normal => (0..replicas).map(|i| message(i, i, sequence, &payload)).collect(),
            Kind::Skip { to } => {
                let mut messages = Vec::with_capacity(replicas as usize);
                for i in 0..replicas {
                    if i == *to {
                        continue;
                    }
                    let message = message(i, i, sequence, &payload);
                    messages.push(message);
                }
                messages
            }
            Kind::Corrupt {
                value: corrupt_payload,
                to,
            } => {
                let mut messages = Vec::with_capacity(replicas as usize);
                for i in 0..replicas {
                    if i == *to {
                        let message = message(i, i, sequence, corrupt_payload);
                        messages.push(message);
                    } else {
                        let message = message(i, i, sequence, &payload);
                        messages.push(message);
                    }
                }
                messages
            }

            Kind::Additional { to, n } => {
                let mut msgs = Vec::with_capacity(1 + *n as usize);
                for j in 0..*n {
                    let payload = payload.clone() + &format!("extra{}", j);
                    let add = message(*to, *to, sequence, &payload);
                    msgs.push(add);
                }
                let message = (0..replicas).map(|i| message(i, i, sequence, &payload));
                msgs.extend(message);
                msgs
            }
        }
    }
}

pub fn count_replies_fm2(program: &[Kind]) -> usize {
    program.iter().map(|k| k.expected_fm2()).sum()
}

fn create_messages(fm2: bool, sequence: u64, payload: String, replicas: u64, kind: Kind) -> Vec<Message<Request>> {
    if fm2 {
        kind.create_messages_fm2(sequence, payload, replicas)
    } else {
        kind.create_messages(sequence, payload, replicas)
    }
}

fn broadcast(source: u64, sequence: u64, n: u64, payload: &str) -> Vec<Message<Request>> {
    // Message::broadcast(source, Request::new(sequence, se(payload)))
    (0..n).map(|i| message(source, i, sequence, payload)).collect()
}

fn message(source: u64, dest: u64, sequence: u64, payload: &str) -> Message<Request> {
    Message::new(source, dest, Request::new(sequence, se(payload)))
}

fn se(payload: &str) -> Bytes {
    let command = Command::transaction(payload.to_owned().into());
    to_vec(&command).unwrap().into()
}

pub fn create_program(fm2: bool, program: Vec<Kind>, replicas: u64) -> Vec<Vec<Message<Request>>> {
    let make_payload = |i: usize| format!("payload{}-{:?}", i, Instant::now());

    let mut program_messages = Vec::with_capacity(program.len());
    for (i, kind) in program.into_iter().enumerate() {
        let payload = make_payload(i);
        let msgs = create_messages(fm2, i as u64, payload, replicas, kind);
        program_messages.push(msgs);
    }
    program_messages
}
