use clap::{command, Arg, Parser, Subcommand};
use core_affinity::CoreId;
use drillx::{equix, Hash};
use futures_util::{future, FutureExt, SinkExt, StreamExt, TryStreamExt};
use std::{
    collections::HashMap,
    ops::Range,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tokio::{signal, time};
use tokio_tungstenite::tungstenite::{Error, Message};
use tracing::*;
use tracing_subscriber::EnvFilter;

use shared::interaction::{ClientResponse, MiningResult, ServerResponse, WorkContent};

use crate::{
    stream::{new_subscribe, StreamCommand, StreamMessage},
    thread::CoreThread,
};

use crate::{
    thread::{CoreResponse, UnitTask},
    watcher::Watcher,
};
use tokio::sync::{broadcast, mpsc, Mutex};

mod stream;
mod thread;
mod watcher;
mod container;

#[derive(Parser, Debug)]
#[command(about, version)]
struct Args {
    #[arg(long, value_name = "SERVER_HOST", help = "Subscribe to mining job server Host")]
    host: String,

    #[arg(
        long,
        value_name = "CORES_COUNT",
        help = "The number of CPU cores to allocate to mining",
        global = true
    )]
    cores: Option<usize>,

    #[arg(long, value_name = "RECONNECT", help = "The number of reconnect times", global = true)]
    reconnect: Option<u32>,

    #[arg(
        long,
        value_name = "SOLANA_PUBKEY",
        help = "The solana wallet pubkey address for Receive Rewards"
    )]
    wallet: String,
}

pub struct UnitResult {
    server_difficulty: u32,
    result: MiningResult,
}

fn init_log() {
    let env_filter = EnvFilter::from_default_env()
        .add_directive("client=debug".parse().unwrap())
        .add_directive("info".parse().unwrap());
    tracing_subscriber::fmt().with_env_filter(env_filter).init();
}

async fn process_stream(
    cmd: &mpsc::Sender<StreamCommand>,
    msg: StreamMessage,
    wallet: String,
    watcher: Arc<Mutex<Watcher>>,
    cores: usize,
) -> Option<Vec<UnitTask>> {
    match msg {
        StreamMessage::WorkContent(data) => {
            let WorkContent {
                id,
                challenge,
                range,
                difficulty,
                deadline,
                work_time,
            } = data;

            cmd.send(StreamCommand::Response(ServerResponse::WorkResponse {
                id,
                wallet,
            }))
                .await
                .ok();

            // {
            //     let mut guard = watcher.lock().await;
            //     guard.new_work(id, difficulty);
            // }

            info!(
                "challenge: `{}`, server difficulty: {difficulty}, deadline: {deadline}",
                bs58::encode(challenge).into_string()
            );

            let mut result = vec![];
            let limit = (range.end - range.start).saturating_div(cores as u64);
            for i in 0..cores as u64 {
                result.push(UnitTask {
                    index: i as u16,
                    id,
                    difficulty,
                    challenge,
                    data: range.start + i * limit..range.start + (i + 1) * limit,
                    stop_time: Instant::now() + Duration::from_secs(work_time),
                });
            }
            Some(result)
        }
        StreamMessage::Ping(ping) => {
            cmd.send(StreamCommand::Ping(ping)).await.ok();
            None
        }
    }
}

async fn process_task(
    resp: CoreResponse,
    watcher: Arc<Mutex<Watcher>>,
    cores: usize,
) -> Option<MiningResult> {
    match resp {
        CoreResponse::Result {
            id,
            index,
            core,
            data,
        } => {
            debug!("[M] >> id: {id}, core: {core}, index: {index}");
            // let mut guard = watcher.lock().await;
            // return guard.best(id, core, index, data);
        }
        CoreResponse::Index {
            id,
            core,
            index,
        } => {
            debug!("[R] >> id: {id}, core: {core}, index: {index}");
            // let mut guard = watcher.lock().await;
            // guard.add(id, core, index);
        }
    }
    None
}

fn start_work(args: Args) -> Vec<JoinHandle<()>> {
    let cores = args.cores.unwrap_or(num_cpus::get());

    let max_retry = args.reconnect.unwrap_or(10);

    info!("Client Starting... Threads: {}, Pubkey: {}", cores, args.wallet);

    let url = format!("ws://{}/worker/{}", args.host, args.wallet);

    info!("connect: [{}]", url);

    let (shutdown, _) = broadcast::channel(1);

    let (core_tx, mut core_rx, core_handler) = CoreThread::start(cores);

    let (stream_tx, mut stream_rx) = new_subscribe(url, max_retry, shutdown.subscribe());

    tokio::spawn({
        let mut notify_shutdown = shutdown.subscribe();
        let mut watcher = Arc::new(Mutex::new(Watcher::new()));
        async move {
            loop {
                tokio::select! {
                    _ = notify_shutdown.recv() => break,
                    Some(msg) = stream_rx.recv() => {
                        if let Some(tasks) = process_stream(&stream_tx, msg, args.wallet.clone(), watcher.clone(), cores).await {

                            for task in tasks {
                                if let Err(err) = core_tx.send(task).await {
                                    error!("fail to send unit task: {err:?}");
                                }
                            }
                        }
                    }
                    Some(res) = core_rx.recv() => {
                        if let Some(res) = process_task(res, watcher.clone(), cores).await {
                            let data = StreamCommand::Response(ServerResponse::MiningResult(res));
                            if let Err(err) = stream_tx.send(data).await{
                                error!("fail to send stream message: {err:?}");
                            }
                        }
                    }
                }
            }
            debug!("[process] async thread shutdown")
        }
    });

    // shutdown drop
    // when one side of the channel is closed, the remaining part will exit
    tokio::spawn(async move {
        signal::ctrl_c().await.expect("failed to listen for Ctrl+C");
        info!("ctrl+c received. start shutdown and wait for all threads to complete their work");
        drop(shutdown)
    });

    core_handler
}

#[tokio::main]
async fn main() {
    init_log();

    let args = Args::parse();

    let core_handler = start_work(args);

    // block main thread, wait for all threads to exit
    for handler in core_handler {
        match handler.join() {
            Ok(_) => {}
            Err(e) => {
                error!("{:?}", e);
            }
        }
    }
}
