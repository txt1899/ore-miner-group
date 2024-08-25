use thiserror::Error;

#[derive(Error, Debug)]
pub enum MinerError {
    #[error("{}")]
    MinerExisted(String),
    #[error("{}")]
    InvalidPubkey(String),
}


pub type MinerResult<T> = Result<T, MinerError>;