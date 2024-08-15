pub mod jito;
pub mod mediator;
pub mod messages;
pub mod scheduler;
pub mod server;
pub mod session;

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub enum MinerStatus {
    Idle     = 0,
    Working  = 1,
    Complete = 2,
    #[default]
    Unknown  = -1,
}

#[derive(Default, Debug, Clone)]
pub struct MinerAccount {
    /// 收益钱包
    pub pubkey: String,
    /// 总核心数
    pub cores: usize,
    /// 工作量
    pub workload: u64,
    /// 贡献度
    pub contribute: u64,
    /// 状态
    pub status: MinerStatus,
}
