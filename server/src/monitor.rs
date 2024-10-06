use actix::{Actor, Addr, StreamHandler};
use actix_http::ws::{Message, ProtocolError};
use actix_web_actors::ws::WebsocketContext;
use tracing::*;
use crate::manager::ManagerActor;

pub use messages as MonitorMessage;

pub struct MonitorActor {
    manager:Addr<ManagerActor>
}

impl MonitorActor {
    pub fn new( manager:Addr<ManagerActor>) -> Self {
        Self {
            manager
        }
    }
}

impl Actor for MonitorActor {
    type Context = WebsocketContext<Self>;
    fn started(&mut self, ctx: &mut Self::Context) {
        // web client connected
    }
}

impl StreamHandler<Result<Message, ProtocolError>> for MonitorActor {
    fn handle(&mut self, item: Result<Message, ProtocolError>, ctx: &mut Self::Context) {
        match item {
            Ok(Message::Text(text)) => {

            }
            Ok(Message::Ping(ping)) => {

            }
            Err(err) => {
                error!("web monitor error ({err})")
            }
            _ => {}
        }
    }
}



pub mod messages {
    pub enum CommandRequest {

    }
}