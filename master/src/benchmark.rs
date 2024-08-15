use drillx::equix;
use solana_rpc_client::spinner;
use std::{io, io::BufRead, sync::Arc, time::Instant};

const TEST_DURATION: i64 = 30;

fn main() {
    let num_cores = num_cpus::get() as u64;
    let challenge = [0; 32];
    let progress_bar = Arc::new(spinner::new_progress_bar());

    progress_bar
        .set_message(format!("算力测试：{}核，花费 {} sec...", num_cores, TEST_DURATION));

    let core_ids = core_affinity::get_core_ids().unwrap();
    let handles: Vec<_> = core_ids
        .into_iter()
        .map(|i| {
            std::thread::spawn({
                move || {
                    let timer = Instant::now();
                    let first_nonce =
                        u64::MAX.saturating_div(num_cores).saturating_mul(i.id as u64);
                    let mut nonce = first_nonce;
                    let mut memory = equix::SolverMemory::new();
                    loop {
                        // Return if core should not be used
                        if (i.id as u64).ge(&num_cores) {
                            return 0;
                        }

                        // Pin to core
                        let _ = core_affinity::set_for_current(i);

                        // Create hash
                        let _hx =
                            drillx::hash_with_memory(&mut memory, &challenge, &nonce.to_le_bytes());

                        // Increment nonce
                        nonce += 1;

                        // Exit if time has elapsed
                        if (timer.elapsed().as_secs() as i64).ge(&TEST_DURATION) {
                            break;
                        }
                    }

                    // Return hash count
                    nonce - first_nonce
                }
            })
        })
        .collect();

    // Join handles and return best nonce
    let mut total_nonces = 0;
    for h in handles {
        if let Ok(count) = h.join() {
            total_nonces += count;
        }
    }

    progress_bar.finish_with_message(format!(
        "算力: {} H/sec",
        total_nonces.saturating_div(TEST_DURATION as u64),
    ));

    let stdin = io::stdin();
    let handle = stdin.lock();
    let _ = handle.lines().next();
}
