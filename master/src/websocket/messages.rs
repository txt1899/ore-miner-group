use std::{collections::HashMap, ops::Range};

use actix::prelude::*;
use drillx::Solution;

use crate::websocket::{scheduler::Scheduler, server::ServerActor, MinerAccount, MinerStatus};

// Server message to Session
#[derive(Message)]
#[rtype(result = "()")]
pub struct Message(pub String);

#[derive(Message)]
#[rtype(result = "()")]
pub struct SpecificMessage {
    pub id: usize,
    pub msg: String,
}

// Session message to Server

/// 新矿工加入
#[derive(Message)]
#[rtype(usize)]
pub struct Connect {
    pub addr: Recipient<Message>,
}

/// 矿工断开
#[derive(Message)]
#[rtype(result = "()")]
pub struct Disconnect {
    pub id: usize,
}

/// 矿工提交算力
#[derive(Message)]
#[rtype(result = "()")]
pub struct UpdateMinerAccount {
    // session id
    pub id: usize,
    // 获取奖励钱包
    pub pubkey: String,
    // cpu核数
    pub cores: usize,
}

#[derive(Default)]
pub struct MineResult {
    pub challenge: [u8; 32],
    pub difficulty: u32,
    pub nonce: u64,
    pub hash: drillx::Hash,
}

// task
///[Scheduler] 将消息转发给 [ServerActor]，根据状态获取矿工
#[derive(Message)]
#[rtype(result = "HashMap<usize, MinerAccount>")]
pub struct FetchMinerByStatus(pub Vec<MinerStatus>);

#[derive(Message)]
#[rtype(usize)]
pub struct AssignTask {
    pub challenge: [u8; 32],
    pub cutoff_time: u64,
    pub min_difficulty: u32,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct UpdateMineResult {
    pub id: usize,
    // 起始随机数 nonce
    pub nonce_range: Range<u64>,
    // 实际工作量
    pub workload: u64,
    pub result: MineResult,
}

#[derive(Message)]
#[rtype(result = "Option<Solution>")]
pub struct GetSolution(pub u32);

#[derive(Message)]
#[rtype(result = "()")]
pub struct NewChallenge(pub [u8; 32]);

#[derive(Message)]
#[rtype(result = "()")]
pub struct ResetMiners;

// mediator

#[derive(Message)]
#[rtype(result = "()")]
pub struct SetServerActor(pub Addr<ServerActor>);

#[derive(Message)]
#[rtype(result = "()")]
pub struct SetTaskActor(pub Addr<Scheduler>);
