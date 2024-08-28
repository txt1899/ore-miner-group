use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
        Arc,
    },
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use tokio::time;
use tokio_tungstenite::tungstenite::Message;
use tracing::*;

use shared::interaction::{ClientResponse, WorkData, ServerResponse, SubmitMiningResult};

use crate::UnitTask;

pub(crate) async fn subscribe_jobs(
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
        info!("connected to server");

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
                    debug!("submit result: {res:?}");
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
                                            debug!("new work received: {work:?}");

                                            let WorkData {
                                                id,
                                                challenge,
                                                work,
                                                difficulty,
                                                cutoff, // TODO need a timestamp
                                                deadline,
                                                work_time
                                            } = work;

                                            info!(
                                                "challenge: `{}` current best difficulty: {difficulty}",
                                                bs58::encode(challenge).into_string()
                                            );

                                            // each core thread will push a certain number of nonce
                                            let limit = (work.end - work.start).saturating_div(count);
                                            for i in 0..count {
                                                if let Err(err) = task_tx.send(UnitTask {
                                                    id,
                                                    difficulty,
                                                    challenge,
                                                    data: work.start+ i * limit .. work.start + (i + 1) * limit,
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
