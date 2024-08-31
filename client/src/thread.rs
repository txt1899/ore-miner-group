use core_affinity::CoreId;
use drillx::{equix, Hash};
use std::{
    ops::Range,
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::Instant,
};
use drillx::equix::SolverMemory;
use tracing::*;

use shared::interaction::MiningResult;

use tokio::sync::{broadcast, mpsc};

pub(crate) struct UnitTask {
    pub index: u16,
    pub id: usize,
    pub difficulty: u32,
    pub challenge: [u8; 32],
    pub data: Range<u64>,
    pub stop_time: Instant,
}

pub enum CoreResponse {
    Result {
        id: usize,
        index: u16,
        core: usize,
        data: MiningResult,
    },
    Index {
        id: usize,
        index: u16,
        core: usize,
    },
}

pub(crate) struct CoreThread {
    pub sender: mpsc::Sender<CoreResponse>,
    pub receiver: Arc<Mutex<mpsc::Receiver<UnitTask>>>,
}

impl CoreThread {
    pub fn start(
        cores: usize,
    ) -> (mpsc::Sender<UnitTask>, mpsc::Receiver<CoreResponse>, Vec<JoinHandle<()>>) {
        // task channel
        let (assign_tx, assign_rx) = mpsc::channel(100);
        // result channel
        let (result_tx, result_rx) = mpsc::channel(100);

        let manager = CoreThread {
            sender: result_tx,
            receiver: Arc::new(Mutex::new(assign_rx)),
        };

        let mut handlers = vec![];
        for id in 0..cores {
            let handler = manager.run(id);
            handlers.push(handler);
        }

        (assign_tx, result_rx, handlers)
    }

    pub(crate) fn run(&self, cid: usize) -> JoinHandle<()> {
        debug!("unit core: {:?}", cid);

        let receiver = self.receiver.clone();
        let sender = self.sender.clone();

        std::thread::spawn(move || {
            // bound thread to core
            let _ = core_affinity::set_for_current(CoreId {
                id: cid,
            });

            let mut memory = equix::SolverMemory::new();

            loop {
                // receive task form channel
                let data = {
                    let mut guard = receiver.lock().unwrap();
                    guard.blocking_recv()
                };

                if let None = data {
                    error!("core: {:?}, task receiver closed", cid,);
                    return;
                }

                if let Some(task) = data {
                    let UnitTask {
                        index,
                        id,
                        difficulty,
                        challenge,
                        data,
                        stop_time,
                    } = task;

                    debug!("id: {id}, core: {cid}, task rage: {data:?}");

                    sender
                        .blocking_send(CoreResponse::Index {
                            id,
                            core: cid,
                            index,
                        })
                        .ok();

                    let mut nonce = data.start;
                    let end = data.end;
                    let mut hashes = 0;

                    let mut best_nonce = nonce;
                    let mut best_difficulty = 0;
                    let mut best_hash = Hash::default();

                    loop {
                        for hx in drillx::hashes_with_memory(
                            &mut memory,
                            &challenge,
                            &nonce.to_le_bytes(),
                        ) {
                            let diff = hx.difficulty();
                            if diff.gt(&best_difficulty) {
                                best_nonce = nonce;
                                best_difficulty = diff;
                                best_hash = hx;
                            }
                            hashes += 1;
                        }

                        if stop_time.le(&Instant::now()) {
                            break;
                        }

                        if nonce.ge(&end) {
                            break;
                        }
                        nonce += 1;
                    }

                    trace!("id: {id}, core: {cid}, difficulty: {best_difficulty}");

                    // if server diff is higher than mine, ignore
                    sender
                        .blocking_send(CoreResponse::Result {
                            id,
                            index,
                            core: cid,
                            data: MiningResult {
                                id,
                                difficulty: best_difficulty,
                                challenge,
                                workload: hashes,
                                nonce: best_nonce,
                                digest: best_hash.d,
                                hash: best_hash.h,
                            },
                        })
                        .ok();
                }
            }
        })
    }
}
