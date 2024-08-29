use std::{
    collections::HashMap,
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering},
        mpsc,
        Mutex,
    },
    time::{Duration, Instant},
};

use clap::{command, Parser, Subcommand};
use core_affinity::CoreId;
use drillx::{equix, Hash};
use futures_util::{future, FutureExt, SinkExt, StreamExt, TryStreamExt};
use tokio::{signal, task::JoinHandle, time};
use tokio_tungstenite::tungstenite::{Error, Message};
use tracing::*;
use tracing_subscriber::EnvFilter;

use shared::interaction::{ClientResponse, ServerResponse, WorkData, WorkResult};

use crate::{manager::CoreManager, stream::subscribe_jobs};

mod manager;
mod stream;

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

fn init_log() {
    let env_filter = EnvFilter::from_default_env()
        //.add_directive("client=debug".parse().unwrap())
        .add_directive("info".parse().unwrap());
    tracing_subscriber::fmt().with_env_filter(env_filter).init();
}

#[tokio::main]
async fn main() {
    init_log();

    let args = Args::parse();

    let max_retry = args.reconnect.unwrap_or(10);

    let cores = args.cores.unwrap_or(num_cpus::get());

    info!("Client Starting... Threads: {}, Pubkey: {}", cores, args.wallet);

    // result channel
    let (result_tx, result_rx) = mpsc::channel();

    // task channel
    let (task_tx, task_rx) = mpsc::channel();
    let arc_task_rx = Arc::new(Mutex::new(task_rx));

    let mut core_handler = vec![];

    let shutdown = Arc::new(AtomicBool::new(false));

    // build work threads
    let manager = CoreManager {
        sender: result_tx.clone(),
        receiver: arc_task_rx.clone(),
    };

    for id in 0..cores {
        let handler = manager.run(id);
        core_handler.push(handler);
    }

    // result send to job server stream
    let (turn_tx, turn_rx) = tokio::sync::mpsc::channel(100);

    // receive tasks result
    tokio::spawn(async move {
        let mut cache: HashMap<usize, Vec<WorkResult>> = HashMap::new();
        while let Ok(rx) = tokio::task::block_in_place(|| result_rx.recv()) {
            let job_id = rx.id;

            cache.entry(rx.id).or_default().push(rx);

            if cache[&job_id].len() == cores {
                if let Some(mut results) = cache.remove(&job_id) {
                    // sort by difficulty, the best result will be first.
                    results.sort_by(|a, b| b.difficulty.cmp(&a.difficulty));

                    // sum workload.
                    let workload = results.iter().map(|r| r.workload).sum::<u64>();

                    // get the best result
                    let mut first = results.remove(0);

                    debug!("job_id: {:?}, difficulty: {}", job_id, first.difficulty);

                    if first.difficulty > 0 {
                        first.workload = workload; // set the total workload
                        if let Err(err) = turn_tx.send(first).await {
                            error!("{}", err);
                        }
                    }
                }
            }
        }
    });

    // subscribe jobs
    let clone_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let url = format!("ws://{}/job/{}", args.host, args.wallet);
        info!("connect: [{}]", url);
        subscribe_jobs(url, clone_shutdown, task_tx, turn_rx, cores as u64, max_retry).await
    });

    tokio::spawn(async move {
        signal::ctrl_c().await.expect("failed to listen for Ctrl+C");
        info!("ctrl+c received. start shutdown and wait for all threads to complete their work");
        // `shutdown` set true, `subscribe_jobs()` will exit and then `task_tx` channel will drop
        shutdown.store(true, Ordering::Relaxed);
    });

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
