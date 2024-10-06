use std::time::{Duration, Instant};

use actix::{
    fut,
    Actor,
    ActorContext,
    ActorFutureExt,
    Addr,
    AsyncContext,
    ContextFutureSpawner,
    Handler,
    Running,
    StreamHandler,
    WrapFuture,
};
use actix_http::ws::{Message, ProtocolError};
use actix_web_actors::{ws, ws::WebsocketContext};
use shared::interaction::{ClientResponse, ServerResponse, WorkContent};
use tracing::*;

pub use messages as WorkerMessage;

use crate::manager::{ManagerActor, ManagerMessage};

const SESSION_DEADLINE: u64 = 20000;

pub struct WorkerActor {
    sid: usize,
    pubkey: String,
    update_at: Instant,
    alive_at: Instant,
    try_ping: bool,
    manager: Addr<ManagerActor>,
}

impl WorkerActor {
    pub fn new(manager: Addr<ManagerActor>, pubkey: String) -> Self {
        Self {
            sid: 0,
            pubkey,
            update_at: Instant::now(),
            alive_at: Instant::now(),
            try_ping: true,
            manager,
        }
    }

    pub fn connect(&mut self, ctx: &mut ws::WebsocketContext<Self>) {
        let addr = ctx.address();

        self.manager
            .send(ManagerMessage::WorkerConnection { pubkey: self.pubkey.clone(), addr })
            .into_actor(self)
            .then(|res, act, ctx| {
                match res {
                    Ok(res) => act.sid = res,
                    _ => ctx.stop(),
                }
                fut::ready(())
            })
            .wait(ctx);
    }
}

impl Actor for WorkerActor {
    type Context = WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        trace!("worker connected");

        self.connect(ctx);

        let interval = Duration::from_millis(8000);
        ctx.run_interval(Duration::from_millis(1000), move |act, ctx| {
            if act.update_at.elapsed() > interval {
                // 防止频繁触发
                act.update_at = act.update_at + Duration::from_millis(1500);
                act.manager.do_send(ManagerMessage::WorkerReady { me: ctx.address() });
            }
            if act.alive_at.elapsed() > Duration::from_millis(SESSION_DEADLINE) {
                if act.try_ping {
                    debug!("try send ping");
                    act.try_ping = false;
                    ctx.ping(&[]);
                } else {
                    error!("worker session in deadline");
                    ctx.close(None);
                    ctx.stop();
                }
            }
        });
    }

    fn stopping(&mut self, _: &mut Self::Context) -> Running {
        self.manager.do_send(ManagerMessage::WorkerDisconnection { sid: self.sid });
        Running::Stop
    }

    fn stopped(&mut self, ctx: &mut Self::Context) {
        warn!("worker disconnected, id: {} ", self.sid);
    }
}

impl StreamHandler<Result<Message, ProtocolError>> for WorkerActor {
    fn handle(&mut self, msg: Result<Message, ProtocolError>, _: &mut Self::Context) {
        self.alive_at = Instant::now();

        match msg {
            Ok(Message::Binary(bin)) => {
                match ServerResponse::from(bin) {
                    ServerResponse::WorkResponse { id, wallet } => {
                        self.update_at = Instant::now();
                        self.manager.do_send(ManagerMessage::WorkSendSuccess { id, wallet });
                    }
                    ServerResponse::MiningResult(data) => {
                        self.manager.do_send(ManagerMessage::WorkResult { data });
                    }
                }
            }
            Ok(Message::Pong(_)) => {
                self.try_ping = true;
                self.alive_at = Instant::now();
                debug!("pong received");
            }
            Err(err) => {
                error!("worker error: {err:?}")
            }
            _ => {}
        }
    }
}

impl Handler<WorkerMessage::WorkContent> for WorkerActor {
    type Result = ();

    fn handle(&mut self, msg: WorkerMessage::WorkContent, ctx: &mut Self::Context) -> Self::Result {
        trace!("worker: work content");
        let messages::WorkContent {
            id,
            miner: _,
            challenge,
            difficulty,
            range,
            deadline,
            work_time,
        } = msg;
        let work = WorkContent { id, challenge, range, difficulty, deadline, work_time };
        ctx.binary(ClientResponse::MiningWork(work));
    }
}

pub mod messages {
    use std::ops::Range;

    use actix::Message;
    use shared::types::MinerKey;

    #[derive(Message)]
    #[rtype(result = "()")]
    pub struct WorkContent {
        pub id: usize,
        pub miner: MinerKey,
        pub challenge: [u8; 32],
        pub difficulty: u32,
        pub range: Range<u64>,
        pub deadline: i64,
        pub work_time: u64,
    }
}
