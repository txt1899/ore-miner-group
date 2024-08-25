use std::ops::Range;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

pub type Challenge = [u8; 32];

#[macro_export]
macro_rules! impl_bytes_conversion {
    ($t:ty) => {
        impl Into<Bytes> for $t {
            fn into(self) -> Bytes {
                let err = concat!(stringify!($t), " failed to serialize");
                let data = bincode::serialize(&self).expect(err);
                Bytes::from(data)
            }
        }

        impl Into<Vec<u8>> for $t {
            fn into(self) -> Vec<u8> {
                let err = concat!(stringify!($t), " failed to serialize");
                bincode::serialize(&self).expect(err)
            }
        }

        impl From<Bytes> for $t {
            fn from(value: Bytes) -> Self {
                let err = concat!(stringify!($t), " failed to deserialize");
                bincode::deserialize::<Self>(&value).expect(err)
            }
        }

        impl From<Vec<u8>> for $t {
            fn from(value: Vec<u8>) -> Self {
                let bytes = Bytes::from(value);
                let err = concat!(stringify!($t), " failed to deserialize");
                bincode::deserialize::<Self>(&bytes).expect(err)
            }
        }
    };
}

/// client -> agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerResponse {
    MiningResult(SubmitMiningResult),
}

impl_bytes_conversion!(ServerResponse);

/// agent -> client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientResponse {
    GetWork(GetWork),
}

impl_bytes_conversion!(ClientResponse);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitMiningResult {
    pub id: usize,
    pub difficulty: u32,
    pub challenge: Challenge,
    pub workload: u64,
    pub nonce: u64,
    pub digest: [u8; 16],
    pub hash: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetWork {
    pub id: usize,
    pub challenge: Challenge,
    pub job: Range<u64>,
    pub difficulty: u32,
    pub cutoff: u64,
    pub work_time: u64,
}

// app Command

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct User {
    pub name: String,
    pub keys: Vec<String>,
}

/// if the `app` does not configure `rpc`, these `rpc` will be used
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LoginResponse {
    pub rpc: String,
    pub jito_url: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NextEpoch {
    pub key: String,
    pub challenge: Challenge,
    pub cutoff: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BlockHash {
    pub key: String,
    pub data: [u8; 32],
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum UserCommand {
    Login(User),
    NextEpoch(NextEpoch),
    BuildInstruction(BlockHash),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CommandResponse {
    pub code: i32,
    pub status: String,
    pub data: Option<String>,
    pub error: Option<String>,
}
