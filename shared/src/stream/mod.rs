use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::stream::{client::*, server::Task};

pub mod client;
pub mod server;

// client
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ClientMessageType {
    MinerState(MinerAccount),
    MineResult(RemoteMineResult),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FromClientData<T> {
    pub status: i32,
    pub msg: T,
}

// server

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerMessageType {
    Task(Task),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FromServerData<T> {
    pub status: i32,
    pub msg: T,
}

pub fn create_server_message<T: Serialize>(status: i32, msg: T) -> serde_json::Result<String> {
    serde_json::to_string(&FromClientData {
        status,
        msg,
    })
}

pub fn create_client_message<T: Serialize>(status: i32, msg: T) -> serde_json::Result<String> {
    serde_json::to_string(&FromServerData {
        status,
        msg,
    })
}
