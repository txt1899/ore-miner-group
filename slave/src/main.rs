use std::{fmt::format, sync::Arc, time::Duration, num::ParseIntError};
use actix_web::web::Bytes;
use awc::{ws, BoxedSocket, ClientResponse};
use clap::{command, Parser, Subcommand};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::{select, sync::mpsc, time::sleep};

use lib_shared::{
    stream,
    stream::{client, server, FromServerData, ServerMessageType},
};
use tracing::{debug, error, info, warn};

use crate::find_hash::find_hash;

mod find_hash;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
const MAX_RECONNECT: u8 = 10;

#[derive(Parser, Debug)]
#[command(about, version)]
struct Args {
    #[arg(long, value_name = "SERVER_URL", help = "Miner server address", global = true)]
    url: Option<String>,

    #[arg(
        long,
        value_name = "CORES_COUNT",
        help = "The number of CPU cores to allocate to mining",
        global = true,
        conflicts_with = "cores_bind"
    )]
    cores: Option<usize>,

    #[arg(
        long,
        value_name = "CORES_BIND",
        help = "The CPU cores to bind to mining",
        global = true,
        conflicts_with = "cores"
    )]
    cores_bind: Option<String>,

    #[arg(long, value_name = "RECONNECT", help = "The number of reconnect times", global = true)]
    reconnect: Option<u8>,

    #[arg(
        long,
        value_name = "SOLANA_WALLET",
        help = "The solana wallet address for Receive Rewards"
    )]
    wallet: String,
}

#[actix_web::main]
async fn main() {
    lib_shared::log::init_log();

    let args = Args::parse();
    let core_ids = parse_cores_bind(&args).expect("Invalid cores_bind");
    let cores = heim_cpu::logical_count().await.unwrap() as usize;
    let host = args.url.expect("Server url can not be empty");

    let mut attempts = 0;
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    let (tx, mut rx) = mpsc::channel::<client::RemoteMineResult>(100);
    let max_reconnect = args.reconnect.unwrap_or(MAX_RECONNECT);
    'break_loop: loop {
        let mut client = awc::Client::new().ws(format!("{host}/ws"));
        match client.connect().await {
            Ok((_, mut ws)) => {
                attempts = 0;

                // 发送客户端信息
                match stream::create_client_message(0, stream::client::MinerAccount {
                    pubkey: args.wallet.clone(),
                    cores: core_ids.len(),
                }) {
                    Ok(msg) => ws.send(ws::Message::Text(msg.into())).await.unwrap(),
                    Err(err) => error!("构建客户端消息失败: {err:?}"),
                }

                loop {
                    select! {
                        _= heartbeat.tick() => {
                            if let Err(err ) = ws.send(ws::Message::Ping(Bytes::new())).await {
                                error!("发送心跳包失败: {err:?}");
                                break;
                            }
                        }
                        Some(data) = rx.recv() => {
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
                                                let core_ids = core_ids.clone();
                                                tokio::task::spawn_blocking(move || {
                                                    let result = find_hash(&core_ids, cores, data);
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
                                Ok(ws::Frame::Close(resion)) => {
                                    if let Some(code) = resion {
                                        error!("关闭连接: {code:?}");
                                        break;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            Err(err) => {
                attempts += 1;
                error!("{err:?}");
                info!("重连中...{}/{}", attempts, max_reconnect);
                sleep(Duration::from_secs(10)).await;
                if attempts >= max_reconnect {
                    break 'break_loop;
                }
            }
        }
    }
}

fn parse_cores_bind(args: &Args) -> Result<Vec<usize>, ParseIntError> {
    if let Some(bind_str) = &args.cores_bind {
        let mut cores = vec![];
        for s in bind_str.split(',') {
            match s.split_once('-') {
                Some((start, end)) => {
                    let start = start.parse::<usize>()?;
                    let end = end.parse::<usize>()?;
                    cores.extend(start..=end);
                }
                None => {
                    cores.push(s.parse::<usize>()?);
                }
            }
        }
        return Ok(cores)
    }
    let cores = args.cores.unwrap_or_else(|| num_cpus::get());
    Ok((0..cores).collect())
}