//! Threads for plug-in code.
//!
//! Plug-ins are built for the stacks their platforms give threads (glibc: 8 MiB, macOS: 8 MiB on the
//! main thread and 512 KiB elsewhere, but their own code assumes plenty), and renders run on the
//! renderer's worker threads (2 MiB): a heavy render (Twixtor's optical flow) overflowed them and
//! crashed. Renders, instance creation (which loads models and initialises runtimes) and the
//! workers of a plug-in's `multiThread` calls run on threads with a plug-in-sized stack
//! ([`PLUGIN_STACK`]) on every OS.
//!
//! Those threads are pooled: a worker that has finished a job waits for the next one (up to
//! [`MAX_IDLE_WORKERS`] wait), so neither a render nor a `multiThread` call starts threads on a hot
//! path, and plug-ins that keep per-thread state (GPU contexts, runtimes, licence checks) are not
//! handed a fresh thread for every frame.

use std::cell::Cell;
use std::sync::mpsc::{Receiver, SendError, Sender, channel, sync_channel};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};

/// Stack size of the threads plug-in code runs on (renders, instance creation, `multiThread`
/// workers): twice glibc's default, which plug-ins are written for.
pub(crate) const PLUGIN_STACK: usize = 16 << 20;

thread_local! {
    /// Set on threads that already have [`PLUGIN_STACK`].
    pub(crate) static ON_PLUGIN_STACK: Cell<bool> = const { Cell::new(false) };
}

/// Work for a plug-in-stack worker.
type Job = Box<dyn FnOnce() + Send>;

/// The workers that are waiting for work (their inboxes), most recently idle last.
fn idle_workers() -> &'static Mutex<Vec<Sender<Job>>> {
    static IDLE: OnceLock<Mutex<Vec<Sender<Job>>>> = OnceLock::new();
    IDLE.get_or_init(|| Mutex::new(Vec::new()))
}

/// The most workers kept waiting. More concurrent jobs get a thread of their own, which ends after
/// its job. (A `multiThread` call with `n` threads uses `n - 1` workers, and renders run several at
/// once.)
const MAX_IDLE_WORKERS: usize = 64;

/// A worker thread: run the jobs sent to it, then (while fewer than [`MAX_IDLE_WORKERS`] wait) wait
/// for the next one.
fn worker_main(first: Arc<Mutex<Option<Job>>>, me: Sender<Job>, inbox: Receiver<Job>) {
    ON_PLUGIN_STACK.with(|c| c.set(true));
    let mut next = first.lock().unwrap_or_else(PoisonError::into_inner).take();
    while let Some(job) = next.take() {
        job();
        {
            let mut idle = idle_workers().lock().unwrap_or_else(PoisonError::into_inner);
            if idle.len() >= MAX_IDLE_WORKERS {
                return;
            }
            idle.push(me.clone());
        }
        next = inbox.recv().ok();
    }
}

/// Start `job` on a plug-in-stack thread and do not wait for it: the most recently idle worker (its
/// thread-local state is the warmest), else a new thread. `Err(job)`: no thread could be started.
fn start(mut job: Job) -> Result<(), Job> {
    loop {
        let worker = idle_workers().lock().unwrap_or_else(PoisonError::into_inner).pop();
        let Some(w) = worker else { break };
        match w.send(job) {
            Ok(()) => return Ok(()),
            // That worker is gone: take the job back and try the next.
            Err(SendError(j)) => job = j,
        }
    }
    // None waiting: start one with this job. The job travels in a shared cell so it is still here
    // when the thread cannot be started.
    let cell = Arc::new(Mutex::new(Some(job)));
    let (me, inbox) = channel::<Job>();
    let started = {
        let cell = cell.clone();
        std::thread::Builder::new().name("ofx-plugin".into()).stack_size(PLUGIN_STACK).spawn(move || worker_main(cell, me, inbox))
    };
    match started {
        Ok(_) => Ok(()),
        Err(_) => match cell.lock().unwrap_or_else(PoisonError::into_inner).take() {
            Some(j) => Err(j),
            None => Ok(()),
        },
    }
}

/// Run `f` on a thread with [`PLUGIN_STACK`] and wait for it (directly when this thread already
/// has one, or when no thread can be started). `None` if `f` panicked.
pub(crate) fn on_plugin_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> Option<R> {
    if ON_PLUGIN_STACK.with(Cell::get) {
        return Some(f());
    }
    let (reply, answer) = sync_channel::<Option<R>>(1);
    // The pooled thread works for this one: the locks this task holds (the library's render lock)
    // are held by it too, so that a nested render it starts does not wait for them.
    let tokens = crate::lock::snapshot();
    let job: Job = Box::new(move || {
        let _adopted = crate::lock::adopt(&tokens);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).ok();
        let _ = reply.send(r);
    });
    if let Err(job) = start(job) {
        // No thread: run here.
        job();
    }
    answer.recv().ok().flatten()
}

/// Run `task(0)`, ..., `task(n - 1)` at the same time and return when all are done: index 0 on this
/// thread, the others on plug-in-stack workers (an index whose thread cannot be started runs on this
/// thread afterwards). `true` if no call panicked. This is what `multiThread` is.
pub(crate) fn run_indexed(n: u32, task: Arc<dyn Fn(u32) -> bool + Send + Sync>) -> bool {
    let n = n.max(1);
    // How many of the others are still running, and whether they all went well.
    let latch = Arc::new((Mutex::new((n as usize - 1, true)), Condvar::new()));
    let mut inline: Vec<Job> = Vec::new();
    for i in 1..n {
        let (task, latch) = (task.clone(), latch.clone());
        let job: Job = Box::new(move || {
            let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task(i))).unwrap_or(false);
            let (state, done) = &*latch;
            let mut st = state.lock().unwrap_or_else(PoisonError::into_inner);
            st.0 = st.0.saturating_sub(1);
            st.1 &= ok;
            done.notify_all();
        });
        if let Err(job) = start(job) {
            inline.push(job);
        }
    }
    let mut ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task(0))).unwrap_or(false);
    for job in inline {
        job();
    }
    let (state, done) = &*latch;
    let mut st = state.lock().unwrap_or_else(PoisonError::into_inner);
    while st.0 > 0 {
        st = done.wait(st).unwrap_or_else(PoisonError::into_inner);
    }
    ok &= st.1;
    ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Recursion that needs a few MiB of stack: more than a 2 MiB worker thread has, less than
    /// [`PLUGIN_STACK`].
    fn deep(n: u32) -> u64 {
        let pad = [n as u8; 32 * 1024];
        let here: u64 = std::hint::black_box(&pad).iter().map(|b| u64::from(*b)).sum();
        if n == 0 { here } else { here + deep(n - 1) }
    }

    /// Render and CreateInstance run on threads with plug-in-sized stacks on every OS, the threads
    /// are reused (a plug-in's thread-local state survives), and a call from such a thread runs
    /// inline instead of waiting for another.
    #[test]
    fn plugin_stack_threads_are_big_pooled_and_not_nested() {
        let here = std::thread::current().id();
        let (there, flagged, total) = on_plugin_stack(|| (std::thread::current().id(), ON_PLUGIN_STACK.with(Cell::get), deep(200))).unwrap();
        assert_ne!(there, here);
        assert!(flagged);
        assert!(total > 0, "{total}");
        // Later calls get an idle worker instead of a new thread each time (the pool is shared with
        // the other tests running at the moment, so not necessarily the same one).
        let ids: HashSet<_> = (0..30).filter_map(|_| on_plugin_stack(|| std::thread::current().id())).collect();
        assert!(ids.len() <= 8, "{} distinct threads for 30 calls", ids.len());
        // A call made on a plug-in-stack thread runs on that thread.
        let inline = on_plugin_stack(|| {
            let me = std::thread::current().id();
            on_plugin_stack(move || std::thread::current().id() == me)
        });
        assert_eq!(inline, Some(Some(true)));
        // Concurrent calls get workers of their own and all finish.
        let handles: Vec<_> = (0..4).map(|i| std::thread::spawn(move || on_plugin_stack(move || deep(20) + i))).collect();
        for (i, h) in handles.into_iter().enumerate() {
            assert!(h.join().unwrap().is_some_and(|v| v >= i as u64));
        }
        // A panic inside the job is reported as `None`, and the pool still works afterwards.
        assert_eq!(on_plugin_stack(|| -> u32 { panic!("a plug-in-side bug") }), None);
        assert_eq!(on_plugin_stack(|| 7), Some(7));
    }

    /// Every index runs once, index 0 here and the others on big-stack workers, and the call returns
    /// only when all are done.
    #[test]
    fn indexed_work_runs_everywhere_and_is_waited_for() {
        let seen = Arc::new(Mutex::new(Vec::<(u32, std::thread::ThreadId, bool)>::new()));
        let finished = Arc::new(AtomicU32::new(0));
        let task: Arc<dyn Fn(u32) -> bool + Send + Sync> = {
            let (seen, finished) = (seen.clone(), finished.clone());
            Arc::new(move |i| {
                // Needs more stack than an ordinary thread has, except on this thread (index 0).
                let deep = if i == 0 { 0 } else { deep(200) };
                std::thread::sleep(std::time::Duration::from_millis(20));
                seen.lock().unwrap().push((i, std::thread::current().id(), ON_PLUGIN_STACK.with(Cell::get) || deep == 0));
                finished.fetch_add(1, Ordering::SeqCst);
                true
            })
        };
        assert!(run_indexed(4, task));
        assert_eq!(finished.load(Ordering::SeqCst), 4, "run_indexed returned before the work was done");
        let seen = seen.lock().unwrap().clone();
        let mut indices: Vec<u32> = seen.iter().map(|s| s.0).collect();
        indices.sort_unstable();
        assert_eq!(indices, [0, 1, 2, 3]);
        assert!(seen.iter().filter(|s| s.0 > 0).all(|s| s.1 != std::thread::current().id() && s.2));
    }

    /// The workers stay: many calls use a handful of threads, not three new ones each.
    #[test]
    fn indexed_work_reuses_its_threads() {
        let ids = Arc::new(Mutex::new(HashSet::new()));
        let task: Arc<dyn Fn(u32) -> bool + Send + Sync> = {
            let ids = ids.clone();
            Arc::new(move |_| {
                ids.lock().unwrap().insert(std::thread::current().id());
                true
            })
        };
        for _ in 0..40 {
            assert!(run_indexed(4, task.clone()));
        }
        let distinct = ids.lock().unwrap().len();
        // This thread plus the workers (some may be shared with other tests running at the same
        // time, never anywhere near one per call: 40 calls would be 120 threads).
        assert!(distinct <= 4 + 16, "{distinct} distinct threads for 40 calls");
    }

    /// A panic in one index does not take the others (or the caller) down, and is reported.
    #[test]
    fn a_panicking_index_is_reported_and_the_rest_finish() {
        let finished = Arc::new(AtomicU32::new(0));
        let task: Arc<dyn Fn(u32) -> bool + Send + Sync> = {
            let finished = finished.clone();
            Arc::new(move |i| {
                if i == 2 {
                    panic!("a plug-in-side bug");
                }
                finished.fetch_add(1, Ordering::SeqCst);
                true
            })
        };
        assert!(!run_indexed(4, task));
        assert_eq!(finished.load(Ordering::SeqCst), 3);
        // Alone, it is one call on this thread.
        assert!(run_indexed(1, Arc::new(|i| i == 0)));
    }
}
