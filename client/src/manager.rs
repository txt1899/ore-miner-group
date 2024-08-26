use std::{
    sync::{mpsc, Arc, Mutex},
    time::Instant,
};

use core_affinity::CoreId;
use drillx::{equix, Hash};
use tracing::{debug, trace};

use shared::interaction::SubmitMiningResult;

use crate::UnitTask;

pub(crate) struct CoreManager {
    pub sender: mpsc::Sender<SubmitMiningResult>,
    pub receiver: Arc<Mutex<mpsc::Receiver<UnitTask>>>,
}

impl CoreManager {
    pub(crate) fn run(&self, id: usize) -> std::thread::JoinHandle<()> {
        debug!("unit core: {:?}", id);

        let receiver = self.receiver.clone();
        let sender = self.sender.clone();

        std::thread::spawn(move || {
            // bound thread to core
            let _ = core_affinity::set_for_current(CoreId {
                id,
            });
            let mut memory = equix::SolverMemory::new();
            loop {
                // receive task form channel
                let data = {
                    let lock = receiver.lock().unwrap();
                    lock.recv()
                };

                if let Err(err) = data {
                    debug!("core: {:?}, error: {}", id, err);
                    return;
                }

                if let Ok(task) = data {
                    let UnitTask {
                        job_id,
                        difficulty,
                        challenge,
                        data,
                        stop_time,
                    } = task;

                    if id == 0 {
                        debug!("core: {id}, task rage: {data:?}");
                    }

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

                    trace!("core: {id} difficulty: {best_difficulty}");

                    // if server diff is higher than mine, ignore
                    sender
                        .send(if best_difficulty > difficulty {
                            SubmitMiningResult {
                                job_id,
                                difficulty: best_difficulty,
                                challenge,
                                workload: hashes,
                                nonce: best_nonce,
                                digest: best_hash.d,
                                hash: best_hash.h,
                            }
                        } else {
                            SubmitMiningResult {
                                job_id,
                                workload: hashes,
                                ..Default::default()
                            }
                        })
                        .ok();
                }
            }
        })
    }
}