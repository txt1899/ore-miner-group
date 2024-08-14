use std::{
    cmp::PartialEq,
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
        Mutex,
    },
};

use actix::{
    dev::MessageResponse,
    fut,
    Actor,
    ActorFutureExt,
    Addr,
    AsyncContext,
    Context,
    ContextFutureSpawner,
    Handler,
    MailboxError,
    MessageResult,
    Recipient,
    ResponseActFuture,
    WeakAddr,
    WrapFuture,
};
use rand::{rngs::ThreadRng, Rng};
use tracing::{error, info, warn};

use lib_shared::stream;

use crate::websocket::{
    mediator::MediatorActor,
    messages,
    scheduler::Scheduler,
    session::SessionActor,
    MinerAccount,
    MinerStatus,
};

pub struct Session {
    addr: Recipient<messages::Message>,
    miner: MinerAccount,
}

/// 矿工服务器，负责管理所有矿工
pub struct ServerActor {
    pub addr: Addr<MediatorActor>,
    pub sessions: HashMap<usize, Session>,
    pub miner_count: Arc<AtomicUsize>,
    pub rng: ThreadRng,
}

impl ServerActor {
    // 广播消息给所有在线矿工
    fn broadcast(&self, msg: &str) {
        self.sessions.iter().for_each(|(id, _)| {
            if let Some(s) = self.sessions.get(id) {
                s.addr.do_send(messages::Message(msg.to_string()))
            }
        })
    }

    // 给指定id矿工发送消息
    fn send_message(&self, id: usize, msg: &str) {
        if let Some(s) = self.sessions.get(&id) {
            s.addr.do_send(messages::Message(msg.to_string()))
        }
    }
}

/// Make actor from `MineServer`
impl Actor for ServerActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.addr.do_send(messages::SetServerActor(ctx.address()))
    }
}

// 处理Connect消息
impl Handler<messages::Connect> for ServerActor {
    type Result = usize;

    fn handle(&mut self, msg: messages::Connect, _: &mut Self::Context) -> Self::Result {
        let id = self.rng.gen::<usize>();
        self.sessions.insert(id, Session {
            addr: msg.addr,
            miner: Default::default(),
        });
        self.miner_count.fetch_add(1, Ordering::SeqCst);
        info!(
            "New miner joined id:{id:?}, current miner count: {:?}",
            self.miner_count.load(Ordering::SeqCst)
        );
        id
    }
}

// 处理Disconnect消息
impl Handler<messages::Disconnect> for ServerActor {
    type Result = ();

    fn handle(&mut self, msg: messages::Disconnect, _: &mut Self::Context) -> Self::Result {
        if self.sessions.remove(&msg.id).is_some() {
            self.miner_count.fetch_sub(1, Ordering::SeqCst);
            info!(
                "A miner disconnected id: {:?}, current miner count: {:?}",
                msg.id,
                self.miner_count.load(Ordering::SeqCst)
            );
        }
    }
}

// 获取指定状态的矿工
impl Handler<messages::FetchMinerByStatus> for ServerActor {
    type Result = MessageResult<messages::FetchMinerByStatus>;

    fn handle(&mut self, msg: messages::FetchMinerByStatus, _: &mut Self::Context) -> Self::Result {
        let status = msg.0;
        MessageResult(
            self.sessions
                .iter()
                .filter_map(|(id, s)| {
                    if status.contains(&s.miner.status) {
                        Some((id.clone(), s.miner.clone()))
                    } else {
                        None
                    }
                })
                .collect(),
        )
    }
}

// 机器算力信息
impl Handler<messages::UpdateMinerAccount> for ServerActor {
    type Result = ();

    fn handle(&mut self, msg: messages::UpdateMinerAccount, _: &mut Self::Context) -> Self::Result {
        if let Some(s) = self.sessions.get_mut(&msg.id) {
            s.miner.pubkey = msg.pubkey;
            s.miner.cores = msg.cores;
            s.miner.status = MinerStatus::Idle;
        }
    }
}

// 一次性将任务分发给所有矿工
impl Handler<messages::AssignTask> for ServerActor {
    type Result = usize;

    fn handle(&mut self, msg: messages::AssignTask, _: &mut Self::Context) -> Self::Result {
        let mut start_nonce = 0_u64;
        let mut success_id = vec![];
        self.sessions.iter().for_each(|(id, session)| {
            match session.miner.status {
                MinerStatus::Working => {
                    //warn!("客户端: {id:?} 工作中")
                }
                MinerStatus::Idle => {
                    let workload = 1_000_000; //((session.miner.hashrate as f32 * 1.2) as u64;
                    match stream::create_client_message(0, stream::server::Task {
                        challenge: msg.challenge,
                        nonce_range: { start_nonce..start_nonce + workload },
                        cutoff_time: msg.cutoff_time,
                        min_difficulty: msg.min_difficulty,
                    }) {
                        Ok(data) => {
                            self.send_message(id.clone(), data.as_str());
                            success_id.push(id.clone());
                            start_nonce += workload;
                        }
                        Err(err) => {
                            error!("创建客户端消息异常：{err:?}");
                        }
                    }
                }
                MinerStatus::Complete => {
                    //warn!("客户端: {id:?} 工作完成")
                }
                MinerStatus::Unknown => {
                    warn!("miner id:{id:?} 未知状态")
                }
            }
        });

        // 更新派送任务的miner状态
        success_id.iter().for_each(|id| {
            if let Some(s) = self.sessions.get_mut(id) {
                s.miner.status = MinerStatus::Working;
            }
        });

        success_id.len()
    }
}

// 将任务结果发送到task actor
impl Handler<messages::UpdateMineResult> for ServerActor {
    type Result = ();

    fn handle(&mut self, msg: messages::UpdateMineResult, _: &mut Self::Context) -> Self::Result {
        if let Some(s) = self.sessions.get_mut(&msg.id) {
            s.miner.status = MinerStatus::Complete;
            s.miner.workload = msg.workload;
            // 贡献度 = 计算量 * 最高难度
            s.miner.contribute = msg.workload.saturating_mul(msg.result.difficulty as u64);
            self.addr.do_send(msg)
        }
    }
}

// 新的挑战重置所有矿工状态
impl Handler<messages::ResetMiners> for ServerActor {
    type Result = ();

    fn handle(&mut self, _: messages::ResetMiners, _: &mut Self::Context) -> Self::Result {
        self.sessions.iter_mut().for_each(|(id, s)| {
            s.miner.status = MinerStatus::Idle;
            s.miner.workload = 0;
            s.miner.contribute = 0;
        })
    }
}
