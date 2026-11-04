//! The bounded worker pool the Argon2 password work runs on, outside the `Core` lock. One hash takes tens of
//! milliseconds and 19 MiB, so a burst of logins must neither stall the lobby nor take all the memory: a few workers
//! take the jobs in order from a short queue, and a job that finds the queue full is refused at once.
// DC-DEFAULT: LG-328 (proposed, project decision) password hashing off the lock on a bounded pool

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{channel, sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};

/// Most workers, whatever the machine: each holds 19 MiB while it works, and the game servers share these cores.
pub const MAX_WORKERS: usize = 4;
/// Jobs waiting for a worker: a player waits some seconds behind a flood at worst, which beats a false refusal.
pub const QUEUE: usize = 256;

/// The queue is full (or the pool is gone): the caller answers "try again", and counts nothing against the player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Busy;

type Job = Box<dyn FnOnce() + Send>;

pub struct PwPool {
    tx: SyncSender<Job>,
}

impl PwPool {
    pub fn new(workers: usize, queue: usize) -> PwPool {
        let (tx, rx) = sync_channel::<Job>(queue);
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..workers.max(1) {
            let rx = Arc::clone(&rx);
            let _ = std::thread::Builder::new()
                .name(format!("pwhash{i}"))
                .spawn(move || work(&rx));
        }
        PwPool { tx }
    }

    /// Runs `f` on a worker and waits for its result, in the order of the calls. Call it without the `Core` lock.
    pub fn run<T, F>(&self, f: F) -> Result<T, Busy>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let (done, wait) = channel();
        self.tx
            .try_send(Box::new(move || {
                let _ = done.send(f());
            }))
            .map_err(|_| Busy)?;
        wait.recv().map_err(|_| Busy)
    }
}

fn work(rx: &Mutex<Receiver<Job>>) {
    loop {
        let job = {
            let g = rx.lock().unwrap_or_else(|e| e.into_inner());
            g.recv()
        };
        match job {
            // a panicking job loses its own answer (its caller gets `Busy`), not this worker
            Ok(j) => {
                let _ = catch_unwind(AssertUnwindSafe(j));
            }
            Err(_) => return,
        }
    }
}

/// The lobby's pool, started by its first job: half the cores, at most [`MAX_WORKERS`].
pub fn global() -> &'static PwPool {
    static POOL: OnceLock<PwPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        PwPool::new((cores / 2).clamp(1, MAX_WORKERS), QUEUE)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn jobs_run_in_order_and_a_full_queue_refuses_at_once() {
        let p = Arc::new(PwPool::new(1, 2));
        let (hold_tx, hold_rx) = channel::<()>();
        let (started_tx, started_rx) = channel::<()>();
        let (order_tx, order_rx) = channel::<u32>();
        // one on the worker, held
        let h0 = {
            let p = Arc::clone(&p);
            std::thread::spawn(move || {
                p.run(move || {
                    started_tx.send(()).unwrap();
                    hold_rx.recv().unwrap();
                })
            })
        };
        started_rx.recv().unwrap();
        // two in the queue, one at a time so that their order is known
        let mut waiting = Vec::new();
        for i in 1..=2u32 {
            let (p, o) = (Arc::clone(&p), order_tx.clone());
            waiting.push(std::thread::spawn(move || p.run(move || o.send(i).unwrap())));
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert_eq!(p.run(|| ()), Err(Busy), "the queue is full");
        hold_tx.send(()).unwrap();
        h0.join().unwrap().unwrap();
        for w in waiting {
            w.join().unwrap().unwrap();
        }
        assert_eq!(order_rx.iter().take(2).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(p.run(|| 7), Ok(7), "free again");
    }

    #[test]
    fn a_panicking_job_keeps_its_worker() {
        let p = PwPool::new(1, 2);
        assert_eq!(p.run(|| -> u32 { panic!("job") }), Err(Busy));
        assert_eq!(p.run(|| 7), Ok(7));
    }
}
