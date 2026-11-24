//! One helper thread, kept for the life of the process, so that two pieces of
//! work can run at the same time without creating a thread per call
//! (`prototype/docs/charlm-optimization.md`, section 3: the output layer on
//! two threads).
//!
//! `join(a, b)` runs `a` on the helper and `b` on the calling thread and
//! returns when both are done, so `a` may borrow the caller's data.

use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread;

/// A piece of work handed to the helper: a function and its data, which live
/// on the caller's stack until the helper reports back.
struct Task {
    run: unsafe fn(*mut ()),
    data: *mut (),
}

// SAFETY: the caller keeps the data alive and does not touch it until the
// helper has finished (see `join`).
unsafe impl Send for Task {}

#[derive(Default)]
struct Slot {
    task: Option<Task>,
    /// Some(panicked) once the task is done.
    done: Option<bool>,
}

pub struct Pool {
    slot: Mutex<Slot>,
    ready: Condvar,
    finished: Condvar,
    /// One caller at a time.
    busy: Mutex<()>,
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
                slot: Mutex::new(Slot::default()),
                ready: Condvar::new(),
                finished: Condvar::new(),
                busy: Mutex::new(()),
            }));
            thread::Builder::new().name("bopogram-helper".into()).spawn(move || pool.serve()).ok()?;
            Some(pool)
        })
    }

    fn serve(&self) {
        loop {
            let task = {
                let mut slot = self.slot.lock().unwrap();
                loop {
                    if let Some(task) = slot.task.take() {
                        break task;
                    }
                    slot = self.ready.wait(slot).unwrap();
                }
            };
            // SAFETY: `join` keeps `data` alive until `done` is set.
            let panicked = catch_unwind(AssertUnwindSafe(|| unsafe { (task.run)(task.data) })).is_err();
            self.slot.lock().unwrap().done = Some(panicked);
            self.finished.notify_one();
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
        {
            let mut slot = self.slot.lock().unwrap();
            slot.done = None;
            slot.task = Some(Task { run: run::<A>, data: &mut a as *mut Option<A> as *mut () });
        }
        self.ready.notify_one();
        let here = catch_unwind(AssertUnwindSafe(b));
        let panicked = {
            let mut slot = self.slot.lock().unwrap();
            loop {
                if let Some(p) = slot.done.take() {
                    break p;
                }
                slot = self.finished.wait(slot).unwrap();
            }
        };
        if let Err(e) = here {
            resume_unwind(e);
        }
        assert!(!panicked, "the helper thread's work failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_runs_both() {
        let Some(pool) = Pool::get() else { return };
        for n in 0..100 {
            let mut left = vec![0u64; 1000];
            let mut right = vec![0u64; 1000];
            pool.join(|| left.iter_mut().enumerate().for_each(|(i, v)| *v = i as u64 * n), || {
                right.iter_mut().enumerate().for_each(|(i, v)| *v = i as u64 + n)
            });
            assert_eq!(left[999], 999 * n);
            assert_eq!(right[999], 999 + n);
        }
    }
}
