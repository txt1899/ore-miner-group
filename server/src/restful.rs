use actix::Addr;
use shared::interaction::{NextEpoch, Peek, RestfulError, Solution, SolutionResponse, User};
use tracing::*;

use crate::{
    manager::{messages as ManagerMessage, ManagerActor, ManagerError},
    user::messages as UserMessage,
};

// #[derive(Debug, Clone)]
pub struct RESTful {
    pub manager: Addr<ManagerActor>,
}

impl RESTful {
    // a http request to check the user login.
    pub async fn user_login(&self, data: User) -> anyhow::Result<(), ManagerError> {
        trace!("User login");
        let User { user: name, miners: keys, version } = data;

        let res = self.manager.send(ManagerMessage::UserLogin { user: name, miners: keys }).await;
        match res {
            Ok(res) => res,
            Err(err) => {
                error!("manager error: {}", err);
                Err(ManagerError::InternalServerError)
            }
        }
    }

    pub async fn next_epoch(&self, data: NextEpoch) -> anyhow::Result<(), ManagerError> {
        trace!("next epoch");
        match self.manager.send(ManagerMessage::FetchUser { user: data.user }).await {
            Ok(res) => {
                let addr = res?;
                addr.do_send(UserMessage::NextEpoch {
                    miner: data.miner,
                    challenge: data.challenge,
                    cutoff: data.cutoff,
                });
            }
            Err(err) => {
                error!("manager actor error: {}", err);
                return Err(ManagerError::InternalServerError);
            }
        }
        Ok(())
    }

    pub async fn get_solution(
        &self,
        data: Solution,
    ) -> anyhow::Result<SolutionResponse, ManagerError> {
        trace!("get solution");
        match self.manager.send(ManagerMessage::FetchUser { user: data.user }).await {
            Ok(res) => {
                let addr = res?;
                match addr
                    .send(UserMessage::Solution { miner: data.miner, challenge: data.challenge })
                    .await
                {
                    Ok(Some(res)) => Ok(res),
                    Ok(None) => Err(ManagerError::MinerNotFound),
                    Err(err) => {
                        error!("user actor error: {}", err);
                        Err(ManagerError::InternalServerError)
                    }
                }
            }
            Err(err) => {
                error!("manager actor error: {}", err);
                Err(ManagerError::InternalServerError)
            }
        }
    }

    pub async fn peek_difficulty(&self, data: Peek) -> anyhow::Result<Vec<i32>, ManagerError> {
        trace!("difficulty peek");
        match self.manager.send(ManagerMessage::FetchUser { user: data.user }).await {
            Ok(res) => {
                let addr = res?;
                match addr.send(UserMessage::Peek { miners: data.miners }).await {
                    Ok(res) => Ok(res),
                    Err(err) => {
                        error!("user actor error: {}", err);
                        Err(ManagerError::InternalServerError)
                    }
                }
            }
            Err(err) => {
                error!("manager actor error: {}", err);
                Err(ManagerError::InternalServerError)
            }
        }
    }
}

pub trait IntoRestfulError {
    fn into_restful_error(self) -> RestfulError;
}

impl IntoRestfulError for ManagerError {
    fn into_restful_error(self) -> RestfulError {
        match self {
            ManagerError::UserNotFound => RestfulError::UserNotFound,
            ManagerError::MinerNotFound => RestfulError::MinerNotFound,
            ManagerError::InternalServerError => RestfulError::InternalServerError,
            err => RestfulError::Custom(err.to_string()),
        }
    }
}
