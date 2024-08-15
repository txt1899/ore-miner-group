use std::collections::HashMap;

use actix::{Actor, ActorFutureExt, Addr, Context, Handler, ResponseActFuture, WrapFuture};

use crate::websocket::{messages, scheduler::Scheduler, server::ServerActor, MinerAccount};

#[derive(Default)]
pub struct MediatorActor {
    server: Option<Addr<ServerActor>>,
    miner: Option<Addr<Scheduler>>,
}

impl Actor for MediatorActor {
    type Context = Context<Self>;
}

impl Handler<messages::SetServerActor> for MediatorActor {
    type Result = ();

    fn handle(&mut self, msg: messages::SetServerActor, _: &mut Self::Context) -> Self::Result {
        self.server.replace(msg.0);
    }
}

impl Handler<messages::SetTaskActor> for MediatorActor {
    type Result = ();

    fn handle(&mut self, msg: messages::SetTaskActor, _: &mut Self::Context) -> Self::Result {
        self.miner.replace(msg.0);
    }
}

// to task actor

impl Handler<messages::UpdateMineResult> for MediatorActor {
    type Result = ();

    fn handle(&mut self, msg: messages::UpdateMineResult, _: &mut Self::Context) -> Self::Result {
        match self.miner {
            None => panic!("miner actor has not been set up"),
            Some(ref addr) => addr.do_send(msg),
        }
    }
}

// to server actor

impl Handler<messages::AssignTask> for MediatorActor {
    type Result = ResponseActFuture<Self, usize>;

    fn handle(&mut self, msg: messages::AssignTask, _: &mut Self::Context) -> Self::Result {
        match self.server {
            None => panic!("server actor has not been set up"),
            Some(ref addr) => {
                Box::pin(addr.send(msg).into_actor(self).map(|res, _, _| res.unwrap()))
            }
        }
    }
}

/// `[消息代理]` 将消息转发给server actor，根据状态获取矿工
impl Handler<messages::FetchMinerByStatus> for MediatorActor {
    type Result = ResponseActFuture<Self, HashMap<usize, MinerAccount>>;

    fn handle(&mut self, msg: messages::FetchMinerByStatus, _: &mut Self::Context) -> Self::Result {
        match self.server {
            None => panic!("server actor has not been set up"),
            Some(ref addr) => {
                Box::pin(addr.send(msg).into_actor(self).map(|res, _, _| res.unwrap()))
            }
        }
    }
}

// [消息代理] 将消息转发给server actor，根据状态获取矿工
impl Handler<messages::ResetMiners> for MediatorActor {
    type Result = ();

    fn handle(&mut self, msg: messages::ResetMiners, _: &mut Self::Context) -> Self::Result {
        match self.server {
            None => panic!("server actor has not been set up"),
            Some(ref addr) => addr.do_send(msg),
        }
    }
}
