use std::ops::Range;

use serde::{Deserialize, Serialize};

// 派发的任务
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    // 挑战随机种子
    pub challenge: [u8; 32],
    // 随机数范围
    pub nonce_range: Range<u64>,
    // 挑战截止时间
    pub cutoff_time: u64,
    // 最小难度
    pub min_difficulty: u32,
}
