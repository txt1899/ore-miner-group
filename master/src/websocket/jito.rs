use crate::{ore::utils::Tip, websocket::messages};
use actix::{Actor, AsyncContext, Context, Handler, WrapFuture};
use actix_web_actors::ws::ProtocolError;
use awc::{BoxedSocket, ClientResponse};
use futures_util::StreamExt;
use rand::seq::SliceRandom;
use std::time::Duration;
use log::warn;
use tokio::time::sleep;
use tracing::{error, info};

pub struct JitoActor {
    pub enable: bool,
    pub tip: u64,
}

impl Actor for JitoActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        if self.enable {
            let this = ctx.address();
            ctx.spawn(
                async move {
                    let url = "ws://bundles-api-rest.jito.wtf/api/v1/bundles/tip_stream";
                    let mut attempts = 0;
                    loop {
                        match awc::Client::new().ws(url).connect().await {
                            Ok((_, mut ws)) => {
                                attempts = 0;
                                while let Some(message) = ws.next().await {
                                    match message {
                                        Ok(awc::ws::Frame::Text(msg)) => {
                                            let text = String::from_utf8(msg.to_vec())
                                                .expect("Invalid UTF-8");
                                            if let Ok(tips) =
                                                serde_json::from_str::<Vec<Tip>>(&text)
                                            {
                                                for item in tips {
                                                    let tip = (item.landed_tips_50th_percentile
                                                        * (10_f64).powf(9.0))
                                                        as u64;
                                                    this.send(messages::TipValue(tip))
                                                        .await
                                                        .expect("更新jito小费失败");
                                                }
                                            }
                                        }
                                        Err(err) => {
                                            error!("{err:?}");
                                            break;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            Err(err) => {
                                error!("{err:?}");
                            }
                        };
                        attempts += 1;
                        warn!("jito重连中({})...", attempts);
                        sleep(Duration::from_secs(5)).await;
                    }
                }
                .into_actor(self),
            );
        }
    }
}

// 小费账户
impl Handler<messages::TipValue> for JitoActor {
    type Result = ();

    fn handle(&mut self, msg: messages::TipValue, _: &mut Self::Context) {
        self.tip = msg.0;
    }
}

// 小费账户
impl Handler<messages::WithTip> for JitoActor {
    type Result = Option<(String, u64)>;

    fn handle(&mut self, _: messages::WithTip, _: &mut Self::Context) -> Self::Result {
        if self.enable {
            let account = [
                "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
                "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
                "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
                "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
                "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
                "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
                "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
                "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
            ]
            .choose(&mut rand::thread_rng())
            .unwrap()
            .to_string();
            Some((account, self.tip))
        } else {
            None
        }
    }
}
