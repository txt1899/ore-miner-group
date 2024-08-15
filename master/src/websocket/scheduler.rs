use std::{
    future::IntoFuture,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

use actix::{Actor, Addr, AsyncContext, Context, Handler, MessageResult, WrapFuture};
use colored::Colorize;
use log::{debug, warn};
use ore_api::state::Proof;
use tokio::time::{sleep, Instant};
use tracing::info;

use crate::{
    ore,
    websocket::{jito::JitoActor, mediator::MediatorActor, messages, MinerStatus},
};

/// 矿工任务调度器
pub struct Scheduler {
    addr: Addr<MediatorActor>,
    jito: Addr<JitoActor>,
    best_result: messages::MineResult,
    best_miner: usize,
    miner: Arc<ore::Miner>,
}

impl Scheduler {
    pub fn new(
        mediator: Addr<MediatorActor>,
        jito: Addr<JitoActor>,
        miner: Arc<ore::Miner>,
    ) -> Self {
        Self {
            addr: mediator,
            jito,
            best_result: Default::default(),
            best_miner: 0,
            miner,
        }
    }
}

impl Actor for Scheduler {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let this = ctx.address();
        let addr = self.addr.clone();
        let jito = self.jito.clone();
        let miner = self.miner.clone();

        self.addr.do_send(messages::SetTaskActor(this.clone()));

        ctx.spawn(
            async move {
                // 检查钱包是否已经注册矿工
                miner.open(jito.clone()).await;

                loop {
                    addr.send(messages::ResetMiners).await.expect("重置矿工出错");

                    let miners = addr
                        .send(messages::FetchMinerByStatus(vec![MinerStatus::Idle]))
                        .await
                        .expect("获取空闲矿工出错");

                    debug!("空闲矿工: {:?}", miners.len());

                    if miners.len() > 0 {
                        let start_time = Instant::now();
                        let clone_this = this.clone();
                        let clone_addr = addr.clone();
                        let find_hash = |proof: Proof, cutoff_time: u64, min_difficulty: u32| {
                            async move {
                                // 更新挑战，清空上一次难度
                                clone_this
                                    .send(messages::NewChallenge(proof.challenge.clone()))
                                    .await
                                    .expect("更新挑战失败");

                                info!(
                                    "{}",
                                    "======================== 新纪元 ========================"
                                        .bold()
                                        .red()
                                );

                                let stop_cutoff_time = if cutoff_time == 0 {
                                    warn!("非活跃状态，派发单任务");
                                    cutoff_time
                                } else {
                                    cutoff_time + 2 // 加上2秒的缓冲时间，防止有矿工未提交结果
                                };

                                debug!("stop_cutoff_time: {stop_cutoff_time:?}");

                                // 派发任务到所有矿工，返回派发的矿工数
                                let cunt = clone_addr
                                    .send(messages::AssignTask {
                                        active: stop_cutoff_time != 0,
                                        challenge: proof.challenge,
                                        cutoff_time,
                                        min_difficulty: min_difficulty.clone(),
                                    })
                                    .await
                                    .expect("派发任务失败");

                                info!("派发矿工数: {cunt:?}");

                                // 等待挖矿结束获取解决方案
                                loop {
                                    sleep(Duration::from_millis(500)).await;

                                    // 截止时间到后获取解决方案
                                    if start_time.elapsed().as_secs().ge(&stop_cutoff_time) {
                                        let result = clone_this
                                            .send(messages::GetSolution(min_difficulty))
                                            .await
                                            .expect("获取解决方案失败");

                                        if result.is_some() {
                                            return result;
                                        }
                                    }

                                    // 可能网络问题导致矿工全部掉线
                                    // 需要进行判断结束本轮挖矿
                                    let miners = clone_addr
                                        .send(messages::FetchMinerByStatus(vec![
                                            MinerStatus::Working,
                                            MinerStatus::Complete,
                                        ]))
                                        .await
                                        .expect("获取挖矿中的矿工出错");

                                    if miners.len() == 0 {
                                        warn!("所有矿工罢工");
                                        return None;
                                    }
                                }
                            }
                        };

                        miner.mine(jito.clone(), find_hash).await
                    } else {
                        sleep(Duration::from_millis(2000)).await;
                    }
                }
            }
            .into_actor(self),
        );
    }
}

// 接收矿工上传挖矿的结果
impl Handler<messages::UpdateMineResult> for Scheduler {
    type Result = ();

    fn handle(&mut self, msg: messages::UpdateMineResult, _: &mut Self::Context) -> Self::Result {
        if self.best_result.challenge == msg.result.challenge {
            if msg.result.difficulty.gt(&self.best_result.difficulty) {
                self.best_result = msg.result;
                self.best_miner = msg.id
            }
        }
    }
}

// 获取最优的解决方案
impl Handler<messages::GetSolution> for Scheduler {
    type Result = MessageResult<messages::GetSolution>;

    fn handle(&mut self, msg: messages::GetSolution, _: &mut Self::Context) -> Self::Result {
        MessageResult(if self.best_result.difficulty.ge(&msg.0) {
            info!(
                "最优哈希: {} (难度: {})",
                bs58::encode(self.best_result.hash.h).into_string(),
                self.best_result.difficulty
            );
            Some((
                self.best_result.difficulty,
                drillx::Solution::new(
                    self.best_result.hash.d,
                    self.best_result.nonce.to_le_bytes(),
                ),
            ))
        } else {
            None
        })
    }
}

// 更新新的挑战
impl Handler<messages::NewChallenge> for Scheduler {
    type Result = ();

    fn handle(&mut self, msg: messages::NewChallenge, _: &mut Self::Context) -> Self::Result {
        self.best_result = Default::default();
        self.best_result.challenge = msg.0;
    }
}
