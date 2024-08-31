use shared::interaction::MiningResult;
use std::{
    ops::Add,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
        Mutex,
    },
    time::Duration,
};
use tokio::sync::Notify;
use tracing::*;
pub struct Container {
    difficulty: u32,
    results: Mutex<Vec<MiningResult>>,
    notify: Arc<Notify>,
    size: usize,
    cores: Mutex<Vec<usize>>,
}

impl Container {
    pub fn new(size: usize, difficulty: u32) -> Self {
        Self {
            difficulty,
            results: Mutex::new(Vec::with_capacity(size)),
            notify: Arc::new(Notify::new()),
            size,
            cores: Mutex::new(vec![]),
        }
    }

    // sort the difficulty of all thread mining results and get the best
    fn sort(&self) -> (u32, MiningResult) {
        let mut guard = self.results.lock().unwrap();
        guard.sort_by(|a, b| b.difficulty.cmp(&a.difficulty));
        (self.difficulty, guard.remove(0))
    }

    // monitor mining result
    pub async fn monitor(&self, timeout_millis: u64) -> (u32, MiningResult) {
        info!("start monitoring mining result");

        let timeout = Duration::from_millis(timeout_millis);

        if let Err(_) = tokio::time::timeout(timeout, self.notify.notified()).await {
            let mut guard = self.cores.lock().unwrap();
            guard.sort();
            error!("wait mining result timeout. working cores: {:?}", *guard);
        }

        self.sort()
    }

    // if some core thread receive work, join this container
    pub fn join(self: &Arc<Self>, core_id: usize) {
        let mut guard = self.cores.lock().unwrap();
        guard.push(core_id);
    }

    // push mining result
    pub fn push(self: &Arc<Self>, value: MiningResult) {
        let mut guard = self.results.lock().unwrap();
        guard.push(value);
        if guard.len() == self.size {
            self.notify.notify_one()
        }
    }
}