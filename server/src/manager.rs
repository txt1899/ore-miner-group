use std::collections::HashMap;

use actix::{
    fut,
    Actor,
    ActorFutureExt,
    Addr,
    AsyncContext,
    Context,
    ContextFutureSpawner,
    Handler,
    ResponseActFuture,
    WrapFuture,
};
use cached::instant::Instant;
use rand::{thread_rng, Rng};
use shared::types::{MinerKey, UserName};
use tracing::{debug, error, info, trace};

pub use messages as ManagerMessage;

use crate::{
    user::{UserActor, UserMessage},
    worker::{WorkerActor, WorkerMessage},
};

#[derive(Debug, Clone)]
struct WorkHistory {
    user: UserName,
    miner: MinerKey,
    create_at: Instant,
}

// impl Default for WorkHistory {
//     fn default() -> Self {
//         Self { user_name: "".to_string(), miner_key: "".to_string(), create_at: Instant::now() }
//     }
// }

pub struct ManagerActor {
    users: HashMap<UserName, Addr<UserActor>>,
    workers: HashMap<usize, Addr<WorkerActor>>,
    work_history: HashMap<usize, WorkHistory>,
    work_id: usize,
}

impl ManagerActor {
    pub fn new() -> Self {
        Self {
            users: Default::default(),
            workers: Default::default(),
            work_history: Default::default(),
            work_id: 0,
        }
    }
}

impl Actor for ManagerActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.run_interval(std::time::Duration::from_millis(5000), |act, _| {
            let list: Vec<_> = act
                .work_history
                .iter()
                .filter_map(|(id, history)| {
                    if history.create_at.elapsed() > std::time::Duration::from_secs(60) {
                        return Some(*id);
                    }
                    None
                })
                .collect();

            debug!("remove work history id: {:?}", list);

            for id in list {
                act.work_history.remove(&id);
            }
        });
    }
}

impl Handler<ManagerMessage::WorkerReady> for ManagerActor {
    type Result = ();

    fn handle(
        &mut self,
        msg: ManagerMessage::WorkerReady,
        ctx: &mut Self::Context,
    ) -> Self::Result {
        trace!("manager: worker ready");
        let ManagerMessage::WorkerReady { me } = msg;
        let users = self.users.clone();
        async move {
            for (name, user) in users.iter() {
                match user.send(UserMessage::FetchWork).await {
                    Ok(Some(data)) => return Some((name.clone(), data)),
                    Ok(None) => info!("{name} no work to do"),
                    Err(err) => {
                        error!("{name} mailbox error. {err:?}")
                    }
                }
            }
            None
        }
        .into_actor(self)
        .then(move |res, act, _| {
            if let Some((
                user,
                UserMessage::WorkResponse {
                    miner,
                    challenge,
                    difficulty,
                    range,
                    deadline,
                    work_time,
                },
            )) = res
            {
                act.work_id += 1;

                act.work_history.insert(act.work_id, WorkHistory {
                    user,
                    miner: miner.clone(),
                    create_at: Instant::now(),
                });

                me.do_send(WorkerMessage::WorkContent {
                    id: act.work_id,
                    miner,
                    challenge,
                    difficulty,
                    range,
                    deadline,
                    work_time,
                })
            }
            fut::ready(())
        })
        .spawn(ctx);
    }
}

impl Handler<ManagerMessage::WorkSendSuccess> for ManagerActor {
    type Result = ();

    fn handle(
        &mut self,
        msg: ManagerMessage::WorkSendSuccess,
        _: &mut Self::Context,
    ) -> Self::Result {
        trace!("manager: work send success");
        info!("work id: {} send to {} success", msg.id, msg.wallet);
    }
}

impl Handler<ManagerMessage::WorkResult> for ManagerActor {
    type Result = ();

    fn handle(&mut self, msg: ManagerMessage::WorkResult, _: &mut Self::Context) -> Self::Result {
        trace!("manager: work result");
        let id = msg.data.id;
        if let Some(history) = self.work_history.remove(&id) {
            if let Some(user) = self.users.get(&history.user) {
                return user
                    .do_send(UserMessage::MiningResult { miner: history.miner, data: msg.data });
            }
            error!("user not found: {}", history.user);
        }
        error!("work history not found: {}", id);
    }
}

impl Handler<ManagerMessage::WorkerConnection> for ManagerActor {
    type Result = usize;

    fn handle(
        &mut self,
        msg: ManagerMessage::WorkerConnection,
        _: &mut Self::Context,
    ) -> Self::Result {
        trace!("manager: worker connection");
        let addr = msg.addr;
        let id = thread_rng().gen::<usize>();
        info!("worker connected wallet: {}, id: {id}", msg.pubkey);
        self.workers.insert(id, addr);
        id
    }
}

impl Handler<ManagerMessage::WorkerDisconnection> for ManagerActor {
    type Result = ();

    fn handle(
        &mut self,
        msg: ManagerMessage::WorkerDisconnection,
        _: &mut Self::Context,
    ) -> Self::Result {
        trace!("manager: worker disconnection");
        self.workers.remove(&msg.sid);
    }
}

impl Handler<ManagerMessage::UserLogin> for ManagerActor {
    type Result = ResponseActFuture<Self, Result<(), ManagerError>>;

    fn handle(&mut self, msg: ManagerMessage::UserLogin, _: &mut Self::Context) -> Self::Result {
        trace!("manager: user login");
        let user = self.users.get(&msg.user).and_then(|addr| Some(addr.clone()));
        Box::pin(
            async move {
                if msg.miners.is_empty() {
                    return Err(ManagerError::InvalidParameters("`keys` is empty".to_string()));
                }
                match user {
                    None => Ok(Some(msg)),
                    Some(user) => {
                        if let Ok(val) =
                            user.send(UserMessage::CheckMinerKey { miners: msg.miners }).await
                        {
                            if !val {
                                return Err(ManagerError::UserAlreadyLogin);
                            }
                        }
                        Ok(None)
                    }
                }
            }
            .into_actor(self)
            .map(|res, act, _| {
                let data = res?;
                if let Some(msg) = data {
                    info!("user login: {}, miner count: {}", msg.user, msg.miners.len());
                    let user = UserActor::new(msg.miners).start();
                    act.users.insert(msg.user, user);
                }
                Ok(())
            }),
        )
    }
}

impl Handler<ManagerMessage::FetchUser> for ManagerActor {
    type Result = Result<Addr<UserActor>, ManagerError>;

    fn handle(&mut self, msg: ManagerMessage::FetchUser, _: &mut Self::Context) -> Self::Result {
        trace!("manager fetch user");
        match self.users.get(&msg.user) {
            None => Err(ManagerError::UserNotFound),
            Some(addr) => Ok(addr.clone()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ManagerError {
    #[error("user already login")]
    UserAlreadyLogin,
    #[error("user not found")]
    UserNotFound,
    #[error("miner not found")]
    MinerNotFound,
    #[error("invalid parameters: {0}")]
    InvalidParameters(String),
    #[error("Internal Server Error")]
    InternalServerError,
}

pub mod messages {
    use actix::{Addr, Message};
    use shared::types::{MinerKey, UserName};

    use crate::{manager::ManagerError, user::UserActor, worker::WorkerActor};

    #[derive(Message)]
    #[rtype(result = "()")]
    pub struct WorkerReady {
        pub me: Addr<WorkerActor>,
    }

    #[derive(Message)]
    #[rtype(result = "()")]
    pub struct WorkSendSuccess {
        pub id: usize,
        pub wallet: String,
    }

    #[derive(Message)]
    #[rtype(result = "()")]
    pub struct WorkResult {
        pub data: shared::interaction::MiningResult,
    }

    #[derive(Message)]
    #[rtype(result = "usize")]
    pub(crate) struct WorkerConnection {
        pub pubkey: String,
        pub addr: Addr<WorkerActor>,
    }

    #[derive(Message)]
    #[rtype(result = "()")]
    pub(crate) struct WorkerDisconnection {
        pub sid: usize,
    }

    #[derive(Message)]
    #[rtype(result = "Result<(), ManagerError>")]
    pub(crate) struct UserLogin {
        pub user: UserName,
        pub miners: Vec<MinerKey>,
    }

    #[derive(Message)]
    #[rtype(result = "Result<Addr<UserActor> ,ManagerError>")]
    pub(crate) struct FetchUser {
        pub user: UserName,
    }
}
