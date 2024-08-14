use std::ops::Range;

use serde::{Deserialize, Serialize};

// CPU核数和算力
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MinerAccount {
    pub pubkey: String,
    pub cores: usize,
}

// hash计算结果
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MineResult {
    /// 挑战随机种子
    pub challenge: [u8; 32],
    /// 随机数
    pub nonce_range: Range<u64>,
    /// 实际工作量
    pub workload: u64,
    /// 最高难度
    pub difficulty: u32,

    // 解决方案参数
    pub nonce: u64,
    pub digest: [u8; 16],
    pub hash: [u8; 32],
}
