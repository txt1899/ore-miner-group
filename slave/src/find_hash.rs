use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Instant,
};

use drillx::{equix, Hash};
use tracing::{debug, field::debug, info};

use lib_shared::stream::{client, server};

pub fn find_hash(cores: usize, task: server::Task) -> client::RemoteMineResult {
    let core_ids = core_affinity::get_core_ids().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let start = Instant::now();

    let handles: Vec<_> = core_ids
        .into_iter()
        .map(|i| {
            std::thread::spawn({
                let mut memory = equix::SolverMemory::new();
                let mut hashrate = 0;
                let total_nonce = task.nonce_range.end - task.nonce_range.start;
                let step = total_nonce.saturating_div(cores as u64);
                let t = task.clone();
                let c = counter.clone();
                move || {
                    // Return if core should not be used
                    if (i.id).ge(&cores) {
                        return (0, 0, Hash::default(), 0);
                    }
                    // Pin to core
                    let _ = core_affinity::set_for_current(i);
                    let timer = Instant::now();
                    let mut nonce = step.saturating_mul(i.id as u64) + t.nonce_range.start;
                    debug!("core: {} start nonce: {}", i.id, nonce);

                    let mut best_nonce = nonce;
                    let mut best_difficulty = 0;
                    let mut best_hash = Hash::default();

                    loop {
                        // Create hash
                        let hashes = drillx::get_hashes_with_memory(
                            &mut memory,
                            &t.challenge,
                            &nonce.to_le_bytes(),
                        );

                        for hx in hashes {
                            let difficulty = hx.difficulty();
                            if difficulty.gt(&best_difficulty) {
                                best_nonce = nonce;
                                best_difficulty = difficulty;
                                best_hash = hx;
                            }
                            hashrate += 1;
                        }

                        // task done
                        if nonce.ge(&t.nonce_range.end) {
                            break;
                        }

                        // Exit if time has elapsed or
                        if nonce % 5 == 0 {
                            if timer.elapsed().as_secs().ge(&(t.cutoff_time)) {
                                if best_difficulty.ge(&t.min_difficulty) {
                                    break;
                                }
                            }
                        }
                        // Increment nonce
                        nonce += 1;
                        c.fetch_add(1, Ordering::SeqCst);
                    }
                    // Return the best nonce
                    (best_nonce, best_difficulty, best_hash, hashrate)
                }
            })
        })
        .collect();

    let mut best_nonce = 0;
    let mut best_difficulty = 0;
    let mut best_hash = Hash::default();
    let mut total_hashrate = 0;
    for h in handles {
        if let Ok((nonce, difficulty, hash, hashrate)) = h.join() {
            total_hashrate += hashrate;
            if difficulty > best_difficulty {
                best_difficulty = difficulty;
                best_nonce = nonce;
                best_hash = hash;
            }
        }
    }

    let hashrate = 1000 * total_hashrate / start.elapsed().as_millis();

    info!("本轮算力: {} H/s", hashrate);

    client::RemoteMineResult {
        challenge: task.challenge,
        nonce_range: task.nonce_range,
        workload: counter.load(Ordering::SeqCst) as u64,
        difficulty: best_difficulty,
        nonce: best_nonce,
        digest: best_hash.d,
        hash: best_hash.h,
    }
}
