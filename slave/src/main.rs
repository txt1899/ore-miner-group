use std::{fmt::format, sync::Arc, time::Duration};

use actix_web::web::Bytes;
use awc::ws;
use clap::{command, Parser, Subcommand};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::{select, sync::mpsc};

use common::{
    stream,
    stream::{client, server, FromServerData, ServerMessageType},
};
use tracing::{debug, error, info, warn};

use crate::find_hash::find_hash;

mod find_hash;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Parser, Debug)]
#[command(about, version)]
struct Args {
    #[arg(long, value_name = "SERVER_URL", help = "Miner server address", global = true)]
    url: Option<String>,

    #[arg(
        long,
        value_name = "CORES_COUNT",
        help = "The number of CPU cores to allocate to mining",
        global = true
    )]
    cores: Option<usize>,

    #[arg(
        long,
        value_name = "SOLANA_WALLET",
        help = "The solana wallet address for Receive Rewards"
    )]
    wallet: String,
}

#[actix_web::main]
async fn main() {
    common::log::init_log();

    let args = Args::parse();
    let host = args.url.expect("Server url can not be empty");
    let cores = args.cores.unwrap_or(num_cpus::get());

    let (res, mut ws) = awc::Client::new().ws(format!("{host}/ws")).connect().await.unwrap();

    match stream::create_client_message(0, stream::client::MinerAccount {
        pubkey: args.wallet,
        cores,
    }) {
        Ok(msg) => ws.send(ws::Message::Text(msg.into())).await.unwrap(),
        Err(err) => error!("构建客户端消息失败: {err:?}"),
    }

    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);

    let (tx, mut rx) = mpsc::channel::<client::MineResult>(100);

    loop {
        select! {
            _= heartbeat.tick() => {
                ws.send(ws::Message::Ping(Bytes::new())).await.unwrap();
                debug!("发送心跳包");
            }
            Some(data) = rx.recv() => {
                debug!("收到结果");
                info!("最高难度: {:?}",data.difficulty);
                 match stream::create_client_message(0, data) {
                    Ok(msg) =>  ws.send(ws::Message::Text(msg.into())).await.unwrap(),
                    Err(err) => error!("构建客户端消息失败: {err:?}"),
                }
            }
            Some(msg) = ws.next() => {
                match msg {
                    Ok(ws::Frame::Text(msg)) => {
                        let text = String::from_utf8(msg.to_vec()).expect("Invalid UTF-8");
                        match serde_json::from_str::<FromServerData<ServerMessageType>>(text.as_str()) {
                            Ok(data) => match data.msg {
                                ServerMessageType::Task(data) => {
                                    info!("新挑战: {:?} 工作量：{}-{} 剩余时间：{:?}",
                                        bs58::encode(data.challenge).into_string(),
                                        data.nonce_range.start,
                                        data.nonce_range.end,
                                        data.cutoff_time);

                                    let clone_tx = tx.clone();

                                    // find_hash 是一个计密集型任务，垄断了事件循环，导致心跳无法及时发送
                                    // 使用 tokio::task::spawn_blocking 将其放到线程池中执行
                                    tokio::task::spawn_blocking(move || {
                                        let result = find_hash(cores, data);
                                        clone_tx.blocking_send(result).expect("发送结果错误");
                                    });
                                }
                            }
                            Err(err) => error!("解析JSON失败: {err:?}\n信息：{text:?}"),
                        }
                    }
                    Ok(ws::Frame::Pong(_)) => {
                        debug!("收到心跳包");
                        // respond to ping probes
                        // ws.send(ws::Message::Pong(Bytes::new())).await.unwrap();
                    }
                    _ => {}
                }
            }
        }
    }
}
