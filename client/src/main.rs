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
    container::Container,
    thread::{CoreResponse, UnitTask},
    watcher::Watcher,
};
use tokio::sync::{broadcast, mpsc, Mutex};

mod container;
mod stream;
mod thread;
mod watcher;

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
    cores: usize,
) -> Option<(Vec<UnitTask>, Arc<Container>)> {
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

            info!(
                "challenge: `{}`, server difficulty: {difficulty}, deadline: {deadline}",
                bs58::encode(challenge).into_string()
            );

            let mut list = vec![];

            let limit = (range.end - range.start).saturating_div(cores as u64);

            let container = Arc::new(Container::new(cores, difficulty));

            for i in 0..cores as u64 {
                list.push(UnitTask {
                    container: Arc::clone(&container),
                    index: i as u16,
                    id,
                    difficulty,
                    challenge,
                    data: range.start + i * limit..range.start + (i + 1) * limit,
                    stop_time: Instant::now() + Duration::from_secs(work_time),
                });
            }

            Some((list, container))
        }
        StreamMessage::Ping(ping) => {
            cmd.send(StreamCommand::Ping(ping)).await.ok();
            None
        }
    }
}

fn start_work(args: Args) -> Vec<JoinHandle<()>> {
    let cores = args.cores.unwrap_or(num_cpus::get());

    let max_retry = args.reconnect.unwrap_or(10);

    info!("Client Starting... Threads: {}, Pubkey: {}", cores, args.wallet);

    let url = format!("ws://{}/worker/{}", args.host, args.wallet);

    info!("connect: [{}]", url);

    let (shutdown, _) = broadcast::channel(1);

    let (core_tx, core_handler) = CoreThread::start(cores);

    let (stream_tx, mut stream_rx) = new_subscribe(url, max_retry, shutdown.subscribe());

    tokio::spawn({
        async move {
            while let Some(msg) = stream_rx.recv().await {
                if let Some((tasks, container)) =
                    process_stream(&stream_tx, msg, args.wallet.clone(), cores).await
                {
                    // assign task
                    for task in tasks {
                        if let Err(err) = core_tx.send(task).await {
                            error!("fail to send unit task: {err:?}");
                        }
                    }

                    // start mining result monitor, set deadline to 12s
                    let cmd_clone = stream_tx.clone();
                    tokio::spawn(
                        async move {
                            let (difficulty, data) = container.monitor(12000).await;
                            if data.difficulty.gt(&difficulty) {
                                info!("id:{} best difficulty: {}", data.id, data.difficulty);
                                cmd_clone.send(StreamCommand::Response(ServerResponse::MiningResult(data))).await.ok();
                            } else {
                                warn!("id:{}, difficulty (remote: {}, local: {})", data.id, difficulty, data.difficulty);
                            }
                        },
                    );
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
