use std::time::{Duration, Instant};

use actix::{prelude::*, Addr, AsyncContext, ContextFutureSpawner, Handler, WrapFuture};
use actix_web_actors::ws;
use drillx::Hash;
use tracing::{debug, error, warn};

use lib_shared::stream::{ClientMessageType, FromClientData};

use crate::websocket::{messages, scheduler::Scheduler, server::ServerActor};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(35);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(30);

pub struct SessionActor {
    pub id: usize,
    pub heart_beat: Instant,
    pub server: Addr<ServerActor>,
}

/// 矿工session，负责矿工的连接、心跳、消息转发
impl SessionActor {
    pub fn heart_beat(&self, ctx: &mut ws::WebsocketContext<Self>) {
        ctx.run_interval(HEARTBEAT_INTERVAL, |act, ctx| {
            // 检查客户端心跳时间
            if Instant::now().duration_since(act.heart_beat) > CLIENT_TIMEOUT {
                // 心跳超时
                warn!("Websocket Client heartbeat failed, disconnecting!");

                // 通知服务端
                act.server.do_send(messages::Disconnect {
                    id: act.id,
                });
                // 停止
                ctx.stop();
                return;
            }

            ctx.ping(b"");
        });
    }

    pub fn join(&mut self, ctx: &mut ws::WebsocketContext<Self>) {
        self.heart_beat(ctx);
        let addr = ctx.address();
        let msg = messages::Connect {
            addr: addr.recipient(),
        };
        self.server
            .send(msg)
            .into_actor(self)
            .then(|res, act, ctx| {
                match res {
                    Ok(res) => act.id = res,
                    _ => ctx.stop(),
                }
                fut::ready(())
            })
            .wait(ctx);
    }

    pub fn leave(&mut self, _: &mut ws::WebsocketContext<Self>) -> Running {
        self.server.do_send(messages::Disconnect {
            id: self.id,
        });
        Running::Stop
    }
}

impl Actor for SessionActor {
    type Context = ws::WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.join(ctx);
    }

    fn stopping(&mut self, ctx: &mut Self::Context) -> Running {
        self.leave(ctx)
    }
}

// 处理消息的发送
impl Handler<messages::Message> for SessionActor {
    type Result = ();

    fn handle(&mut self, msg: messages::Message, ctx: &mut Self::Context) {
        ctx.text(msg.0);
    }
}

impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for SessionActor {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        let msg = match msg {
            Err(_) => {
                ctx.stop();
                return;
            }
            Ok(msg) => msg,
        };

        match msg {
            ws::Message::Ping(msg) => {
                self.heart_beat = Instant::now();
                ctx.pong(&msg);
                debug!("收到心跳包: {:?}", self.heart_beat);
            }
            ws::Message::Pong(_) => {
                self.heart_beat = Instant::now();
            }
            ws::Message::Text(text) => {
                match serde_json::from_str::<FromClientData<ClientMessageType>>(&text) {
                    Ok(data) => {
                        match data.msg {
                            // 矿工算力
                            ClientMessageType::MinerState(data) => {
                                self.server.do_send(messages::UpdateMinerAccount {
                                    id: self.id,
                                    pubkey: data.pubkey,
                                    cores: data.cores,
                                });
                            }
                            // 任务结果
                            ClientMessageType::MineResult(data) => {
                                debug!("mine result response: {data:?}");

                                let result = messages::MineResult {
                                    challenge: data.challenge,
                                    difficulty: data.difficulty,
                                    nonce: data.nonce,
                                    hash: Hash {
                                        d: data.digest,
                                        h: data.hash,
                                    },
                                };

                                self.server.do_send(messages::UpdateMineResult {
                                    id: self.id,
                                    nonce_range: data.nonce_range,
                                    workload: data.workload,
                                    result,
                                });
                            }
                        }
                    }
                    Err(err) => error!("解析JSON失败: {err:?}\n信息：{text:?}"),
                }
            }

            ws::Message::Close(reason) => {
                ctx.close(reason);
                ctx.stop();
            }
            ws::Message::Continuation(_) => {
                ctx.stop();
            }
            ws::Message::Binary(_) => println!("Unexpected binary"),
            ws::Message::Nop => (),
        }
    }
}
