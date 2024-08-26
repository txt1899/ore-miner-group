use clap::{command, Parser, Subcommand};
use core_affinity::CoreId;
use drillx::{equix, Hash};
use futures_util::{future, FutureExt, SinkExt, StreamExt, TryStreamExt};
use shared::interaction::{ClientResponse, GetWork, ServerResponse, SubmitMiningResult};
use std::{
    ops::Range,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
        Arc,
        Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{signal, task::JoinHandle, time};
use tokio_tungstenite::tungstenite::{Error, Message};
use tracing::*;
use tracing_subscriber::EnvFilter;

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

pub struct UnitCore {
    id: usize,
    sender: mpsc::Sender<SubmitMiningResult>,
    receiver: Arc<Mutex<mpsc::Receiver<UnitTask>>>,
}

fn init_log() {
    let env_filter = EnvFilter::from_default_env()
        .add_directive("client=trace".parse().unwrap())
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
    for id in 0..cores {
        let unit = UnitCore {
            id,
            sender: result_tx.clone(),
            receiver: arc_task_rx.clone(),
        };

        let handler = unit.run();
        core_handler.push(handler);
    }

    // result send to job server stream
    let (turn_tx, turn_rx) = tokio::sync::mpsc::channel(100);

    // receive tasks result
    tokio::spawn(async move {
        while let Ok(rx) = result_rx.recv() {
            if let Err(err) = turn_tx.send(rx).await {
                error!("{}", err);
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

impl UnitCore {
    fn run(self) -> std::thread::JoinHandle<()> {
        debug!("unit core: {:?}", self.id);

        std::thread::spawn(move || {
            // bound thread to core
            let _ = core_affinity::set_for_current(CoreId {
                id: self.id,
            });
            let mut memory = equix::SolverMemory::new();
            loop {
                // receive task form channel
                let data = {
                    let lock = self.receiver.lock().unwrap();
                    lock.recv()
                };

                if let Err(err) = data {
                    debug!("core: {:?}, error: {}", self.id, err);
                    return;
                }

                if let Ok(task) = data {
                    let UnitTask {
                        id,
                        difficulty,
                        challenge,
                        data,
                        stop_time,
                    } = task;

                    info!(
                        "challenge: `{}` current best difficulty: {difficulty}",
                        bs58::encode(challenge).into_string()
                    );

                    let mut nonce = data.start;
                    let end = data.end;

                    let mut best_nonce = nonce;
                    let mut best_difficulty = 0;
                    let mut best_hash = Hash::default();

                    loop {
                        for hx in drillx::hashes_with_memory(
                            &mut memory,
                            &challenge,
                            &nonce.to_le_bytes(),
                        ) {
                            let difficulty = hx.difficulty();
                            if difficulty.gt(&best_difficulty) {
                                best_nonce = nonce;
                                best_difficulty = difficulty;
                                best_hash = hx;
                            }
                        }

                        if nonce % 5 == 0 && stop_time.le(&Instant::now()) {
                            break;
                        }

                        if nonce.ge(&end) {
                            break;
                        }

                        std::thread::sleep(Duration::from_millis(10));
                        nonce += 1;
                    }

                    self.sender
                        .send(SubmitMiningResult {
                            id,
                            difficulty: best_difficulty,
                            challenge,
                            workload: nonce - end,
                            nonce: best_nonce,
                            digest: best_hash.d,
                            hash: best_hash.h,
                        })
                        .ok();
                }
            }
        })
    }
}

async fn subscribe_jobs(
    url: String,
    shutdown: Arc<AtomicBool>,
    task_tx: mpsc::Sender<UnitTask>,
    mut turn_rx: tokio::sync::mpsc::Receiver<SubmitMiningResult>,
    count: u64,
    max_retry: u32,
) {
    let mut attempts = 0;

    let watch_shutdown = || {
        async {
            time::sleep(Duration::from_secs(1)).await;
            shutdown.load(Ordering::SeqCst)
        }
    };

    'done: loop {
        attempts += 1;

        if shutdown.load(Ordering::SeqCst) {
            warn!("shutdown flag is set. exiting...");
            break 'done;
        }

        // connect to server
        let stream = match tokio_tungstenite::connect_async(&url).await {
            Ok((ws_stream, _)) => ws_stream,
            Err(err) => {
                error!("fail to connect to sever: {err:#}");
                if attempts >= max_retry {
                    break;
                }
                info!("retry...({attempts}/{max_retry})");

                tokio::select! {
                    _= watch_shutdown() => {
                        if shutdown.load(Ordering::SeqCst) {
                            warn!("shutdown flag is set. exiting...");
                            break 'done;
                        }
                    }
                    _= tokio::time::sleep(Duration::from_secs(10)) => {}
                }
                continue;
            }
        };

        // connection successful reset attempt
        attempts = 0;

        let (mut write, mut read) = stream.split();
        loop {
            tokio::select! {
                _= watch_shutdown() => {
                    if shutdown.load(Ordering::SeqCst) {
                        warn!("shutdown flag is set. exiting...");
                        break 'done;
                    }
                }
                // the best result will be sent to the server
                res = turn_rx.recv() => {
                    match res {
                        Some(result) => {
                            let data = ServerResponse::MiningResult(result);
                            write.send(Message::Binary(data.into())).await.ok();
                        }
                        None => {
                            error!("turn rx closed");
                        }
                    }
                },
                // new job from server
                Some(res) = read.next() => {
                    match res {
                        Ok(message) => {
                            match message {
                                Message::Binary(bin) => {
                                    let data = ClientResponse::from(bin);
                                    match data {
                                        ClientResponse::GetWork(work) => {

                                            let GetWork {
                                                id,
                                                challenge,
                                                job,
                                                difficulty,
                                                cutoff, // TODO need a timestamp
                                                work_time
                                            } = work;

                                            // each core thread will push a certain number of nonce
                                            let limit = (job.end - job.start).saturating_div(count);
                                            for i in 0..count {
                                                if let Err(err) = task_tx.send(UnitTask {
                                                    id,
                                                    difficulty,
                                                    challenge,
                                                    data: i * limit..(i + 1) * limit,
                                                    stop_time: Instant::now() + Duration::from_secs(work_time),
                                                }) {
                                                    error!("fail to send unit task: {err:#}");
                                                }
                                            }
                                        }
                                    }
                                }
                                Message::Ping(ping) => {
                                    if let Err(err) = write.send(Message::Pong(ping)).await {
                                        error!("fail to send pong: {err:#}");
                                        break;
                                    }
                                }
                                _ => {}
                            }
                        }
                        Err(err) => {
                            error!("fail to read from sever: {err:#}");
                            break;
                        }
                    }
                }
            }
        }

        error!("server disconnected, retries in 10 seconds");
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    trace!("stream done");
}
