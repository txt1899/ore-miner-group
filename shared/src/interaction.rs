use actix_web::{http::StatusCode, HttpResponse, ResponseError};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::ops::Range;
use thiserror::Error;

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

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct SubmitMiningResult {
    pub job_id: usize,
    pub difficulty: u32,
    pub challenge: Challenge,
    pub workload: u64,
    pub nonce: u64,
    pub digest: [u8; 16],
    pub hash: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetWork {
    pub job_id: usize,
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
    BuildTransaction(BlockHash),
}

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
    #[error("key not found")]
    KeyNotFound,
    #[error("solution not found")]
    SolutionNotFound,
    #[error("Internal Server Error")]
    InternalServerError,
}

impl RestfulError {
    fn get_code(&self) -> i32 {
        match self {
            RestfulError::UserExisted => -10000,
            RestfulError::KeyNotFound => -10001,
            RestfulError::SolutionNotFound => -10002,
            RestfulError::InternalServerError => -30000,
        }
    }
}

impl ResponseError for RestfulError {
    fn status_code(&self) -> StatusCode {
        StatusCode::OK
    }

    fn error_response(&self) -> HttpResponse {
        let code = self.get_code();
        let error = self.to_string();
        match self {
            RestfulError::UserExisted => {
                HttpResponse::NotFound().json(RestfulResponse::<()>::error(error, code))
            }
            RestfulError::KeyNotFound => {
                HttpResponse::NotFound().json(RestfulResponse::<()>::error(error, code))
            }
            RestfulError::InternalServerError => {
                HttpResponse::InternalServerError().json(RestfulResponse::<()>::error(error, code))
            }
            RestfulError::SolutionNotFound => {
                HttpResponse::BadRequest().json(RestfulResponse::<()>::error(error, code))
            }
        }
    }
}
