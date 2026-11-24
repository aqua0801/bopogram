//! One helper thread, kept for the life of the process, so that two pieces of
//! work can run at the same time without creating a thread per call
//! (`prototype/docs/charlm-optimization.md`, section 3).
//!
//! `join(a, b)` runs `a` on the helper and `b` on the calling thread and
//! returns when both are done, so `a` may borrow the caller's data.
//!
//! Handing work over has to be quick: one re-ranking makes a few dozen joins.
//! Waking a sleeping thread takes tens of microseconds on Windows, so after
//! each piece of work the helper keeps checking for the next one for a short
//! while (`SPIN`) before it sleeps; the caller does the same while it waits
//! for the helper. Between keystrokes both sleep.

use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

/// How long to keep checking before sleeping.
const SPIN: Duration = Duration::from_micros(200);

/// A piece of work handed to the helper: a function and its data, which live
/// on the caller's stack until the helper reports back.
struct Task {
    run: unsafe fn(*mut ()),
    data: *mut (),
}

// SAFETY: the caller keeps the data alive and does not touch it until the
// helper has finished (see `join`).
unsafe impl Send for Task {}

pub struct Pool {
    task: Mutex<Option<Task>>,
    /// A task is waiting in `task`.
    pending: AtomicBool,
    /// The last task is done.
    done: AtomicBool,
    /// It panicked.
    failed: AtomicBool,
    lock: Mutex<()>,
    wake: Condvar,
    /// One caller at a time.
    busy: Mutex<()>,
}

/// Check `flag` for a while, then sleep on `cv` until it is set.
fn wait_for(flag: &AtomicBool, lock: &Mutex<()>, cv: &Condvar) {
    let start = Instant::now();
    while !flag.load(Ordering::Acquire) {
        if start.elapsed() > SPIN {
            let mut guard = lock.lock().unwrap();
            while !flag.load(Ordering::Acquire) {
                guard = cv.wait_timeout(guard, Duration::from_millis(50)).unwrap().0;
            }
            return;
        }
        std::hint::spin_loop();
    }
}

impl Pool {
    /// The helper, if this computer has more than one core.
    pub fn get() -> Option<&'static Pool> {
        static POOL: OnceLock<Option<&'static Pool>> = OnceLock::new();
        *POOL.get_or_init(|| {
            if thread::available_parallelism().map_or(1, |n| n.get()) < 2 {
                return None;
            }
            let pool: &'static Pool = Box::leak(Box::new(Pool {
                task: Mutex::new(None),
                pending: AtomicBool::new(false),
                done: AtomicBool::new(false),
                failed: AtomicBool::new(false),
                lock: Mutex::new(()),
                wake: Condvar::new(),
                busy: Mutex::new(()),
            }));
            thread::Builder::new().name("bopogram-helper".into()).spawn(move || pool.serve()).ok()?;
            Some(pool)
        })
    }

    fn signal(&self, flag: &AtomicBool) {
        flag.store(true, Ordering::Release);
        // Taking the lock orders this with a sleeper's check (no lost wake-up).
        drop(self.lock.lock().unwrap());
        self.wake.notify_all();
    }

    fn serve(&self) {
        loop {
            wait_for(&self.pending, &self.lock, &self.wake);
            self.pending.store(false, Ordering::Relaxed);
            let Some(task) = self.task.lock().unwrap().take() else { continue };
            // SAFETY: `join` keeps `data` alive until `done` is set.
            let failed = catch_unwind(AssertUnwindSafe(|| unsafe { (task.run)(task.data) })).is_err();
            self.failed.store(failed, Ordering::Relaxed);
            self.signal(&self.done);
        }
    }

    /// Run `a` on the helper and `b` here; return when both are done.
    pub fn join<A: FnOnce() + Send, B: FnOnce()>(&self, a: A, b: B) {
        unsafe fn run<A: FnOnce()>(data: *mut ()) {
            let a = &mut *(data as *mut Option<A>);
            if let Some(a) = a.take() {
                a();
            }
        }
        let _one = self.busy.lock().unwrap();
        let mut a = Some(a);
        self.done.store(false, Ordering::Relaxed);
        *self.task.lock().unwrap() = Some(Task { run: run::<A>, data: &mut a as *mut Option<A> as *mut () });
        self.signal(&self.pending);
        let here = catch_unwind(AssertUnwindSafe(b));
        wait_for(&self.done, &self.lock, &self.wake);
        if let Err(e) = here {
            resume_unwind(e);
        }
        assert!(!self.failed.load(Ordering::Relaxed), "the helper thread's work failed");
    }

    /// Split `0..n` in two at `mid` and run `f` on both parts at once.
    pub fn halves(&self, mid: usize, n: usize, f: impl Fn(std::ops::Range<usize>) + Sync) {
        self.join(|| f(0..mid), || f(mid..n));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_runs_both() {
        let Some(pool) = Pool::get() else { return };
        for n in 0..2000 {
            let mut left = vec![0u64; 1000];
            let mut right = vec![0u64; 1000];
            pool.join(|| left.iter_mut().enumerate().for_each(|(i, v)| *v = i as u64 * n), || {
                right.iter_mut().enumerate().for_each(|(i, v)| *v = i as u64 + n)
            });
            assert_eq!(left[999], 999 * n);
            assert_eq!(right[999], 999 + n);
            if n % 500 == 0 {
                // Long enough for the helper to go to sleep.
                thread::sleep(Duration::from_millis(2));
            }
        }
    }
}
