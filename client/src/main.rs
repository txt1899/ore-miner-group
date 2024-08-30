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
        Mutex,
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

use tokio::sync::{broadcast, mpsc};

mod stream;
mod thread;

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

pub struct UnitTask {
    id: usize,
    difficulty: u32,
    challenge: [u8; 32],
    data: Range<u64>,
    stop_time: Instant,
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
    message: &mut mpsc::Receiver<StreamMessage>,
    wallet: String,
    cores: usize,
) -> Option<Vec<UnitTask>> {
    if let Some(msg) = message.recv().await {
        if let Some(data) = match msg {
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

                info!(
                    "challenge: `{}`, server difficulty: {difficulty}, deadline: {deadline}",
                    bs58::encode(challenge).into_string()
                );

                let mut result = vec![];
                let limit = (range.end - range.start).saturating_div(cores as u64);
                for i in 0..cores as u64 {
                    result.push(UnitTask {
                        id,
                        difficulty,
                        challenge,
                        data: range.start + i * limit..range.start + (i + 1) * limit,
                        stop_time: Instant::now() + Duration::from_secs(work_time),
                    });
                }
                return Some(result);
            }
            StreamMessage::Ping(ping) => Some(StreamCommand::Ping(ping)),
        } {
            cmd.send(data).await.ok();
        }
    }
    None
}

async fn process_task(
    result: &mut mpsc::Receiver<MiningResult>,
    cache: &mut HashMap<usize, Vec<MiningResult>>,
    cores: usize,
) -> Option<MiningResult> {
    if let Some(res) = result.recv().await {
        let wid = res.id;
        cache.entry(wid).or_default().push(res);
        if cache[&wid].len() == cores as usize {
            if let Some(mut vec) = cache.remove(&wid) {
                // sort by difficulty, the best result will be first.
                vec.sort_by(|a, b| b.difficulty.cmp(&a.difficulty));
                // sum workload.
                let workload = vec.iter().map(|r| r.workload).sum::<u64>();

                // get the best result
                let mut first = vec.remove(0);
                return Some(first);
            }
        }
    }
    None
}

fn start_work(args: Args) -> Vec<JoinHandle<()>> {
    let cores = args.cores.unwrap_or(num_cpus::get());

    let max_retry = args.reconnect.unwrap_or(10);

    info!("Client Starting... Threads: {}, Pubkey: {}", cores, args.wallet);

    let mut cache: HashMap<usize, Vec<MiningResult>> = HashMap::new();

    let url = format!("ws://{}/worker/{}", args.host, args.wallet);

    let (shutdown, _) = broadcast::channel(1);

    let (core_tx, mut core_rx, core_handler) = CoreThread::start(cores);

    let (stream_tx, mut stream_rx) = new_subscribe(url, max_retry, shutdown.subscribe());

    tokio::spawn({
        let mut notify_shutdown = shutdown.subscribe();
        async move {
            loop {
                tokio::select! {
                    _ = notify_shutdown.recv() => break,
                    Some(tasks) = process_stream(&stream_tx, &mut stream_rx, args.wallet.clone(), cores) => {
                        for task in tasks {
                            if let Err(err) = core_tx.send(task).await {
                                 error!("fail to send unit task: {err:?}");
                            }
                        }
                   }
                    Some(res) = process_task(&mut core_rx, &mut cache, cores) => {
                        if let Err(err) = stream_tx.send(StreamCommand::Response(ServerResponse::MiningResult(res))).await{
                            error!("fail to send unit task: {err:?}");
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

    let shutdown = Arc::new(AtomicBool::new(false));

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
