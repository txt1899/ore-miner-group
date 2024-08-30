use std::ops::Range;

use actix_web::{http::StatusCode, HttpResponse, ResponseError};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::{MinerKey, UserName};

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

/// client -> server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerResponse {
    /// client notify the server of the received task
    WorkResponse {
        id: usize,
        wallet: String,
    },
    /// client send the work result to server
    MiningResult(MiningResult),
}

impl_bytes_conversion!(ServerResponse);

/// server -> client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientResponse {
    /// server send the work to client
    MiningWork(WorkContent),
}

impl_bytes_conversion!(ClientResponse);

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct MiningResult {
    pub id: usize,
    pub difficulty: u32,
    pub challenge: [u8; 32],
    pub workload: u64,
    pub nonce: u64,
    pub digest: [u8; 16],
    pub hash: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkContent {
    pub id: usize,
    pub challenge: [u8; 32],
    pub range: Range<u64>,
    pub difficulty: u32,
    pub deadline: i64,
    pub work_time: u64,
}

// app Command

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct User {
    pub user: UserName,
    pub miners: Vec<MinerKey>,
}

/// if the `app` does not configure `rpc`, these `rpc` will be used
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LoginResponse {
    pub rpc: String,
    pub jito_url: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NextEpoch {
    pub user: UserName,
    pub miner: MinerKey,
    pub challenge: [u8; 32],
    pub cutoff: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Solution {
    pub user: UserName,
    pub miner: MinerKey,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Peek {
    pub user: UserName,
    pub miners: Vec<MinerKey>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SolutionResponse {
    pub nonce: u64,
    pub digest: [u8; 16],
    pub hash: [u8; 32],
    pub difficulty: u32,
}

// #[derive(Serialize, Deserialize, Debug, Clone)]
// #[serde(untagged)]
// pub enum UserCommand {
//     Login(User),
//     NextEpoch(NextEpoch),
//     GetSolution(SolutionResponse),
// }

// restful api

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RestfulResponse<T> {
    pub code: i32,
    pub data: Option<T>,
    pub message: Option<String>,
}

impl<T> RestfulResponse<T> {
    pub fn success(data: T) -> Self {
        RestfulResponse {
            code: 200,
            data: Some(data),
            message: None,
        }
    }

    pub fn error(message: String, code: i32) -> Self {
        RestfulResponse {
            code,
            data: None,
            message: Some(message),
        }
    }
}

#[derive(Debug, Error)]
pub enum RestfulError {
    #[error("user is existed")]
    UserExisted,
    #[error("user not found")]
    UserNotFound,
    #[error("miner not found")]
    MinerNotFound,
    #[error("solution not found")]
    SolutionNotFound,
    #[error("Internal Server Error")]
    InternalServerError,
    #[error("custom error: {0}")]
    Custom(String),
}

impl RestfulError {
    fn get_code(&self) -> i32 {
        match self {
            RestfulError::UserExisted => -10000,
            RestfulError::UserNotFound => -10001,
            RestfulError::MinerNotFound => -10002,
            RestfulError::SolutionNotFound => -10003,
            RestfulError::InternalServerError => -30000,
            RestfulError::Custom(_) => -90000,
        }
    }

    fn get_status_code(&self) -> StatusCode {
        match self {
            RestfulError::UserExisted => StatusCode::FOUND,
            RestfulError::UserNotFound => StatusCode::NOT_FOUND,
            RestfulError::MinerNotFound => StatusCode::NOT_FOUND,
            RestfulError::SolutionNotFound => StatusCode::NOT_FOUND,
            RestfulError::InternalServerError => StatusCode::INTERNAL_SERVER_ERROR,
            RestfulError::Custom(_) => StatusCode::BAD_REQUEST,
        }
    }
}

impl ResponseError for RestfulError {
    fn status_code(&self) -> StatusCode {
        self.get_status_code()
    }

    fn error_response(&self) -> HttpResponse {
        let code = self.get_code();
        let error_message = self.to_string();
        HttpResponse::build(self.status_code())
            .json(RestfulResponse::<()>::error(error_message, code))
    }
}
