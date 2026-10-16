//! A reentrant lock for the plug-ins that declare themselves render-thread-unsafe.
//!
//! A plug-in with `kOfxImageEffectPluginRenderThreadSafety` = `OfxImageEffectRenderUnsafe` must not
//! run on two threads at once, so every render (and padding query) of the library's plug-ins holds
//! one lock for its whole duration. That lock cannot be an ordinary mutex: while the plug-in
//! renders, its `clipGetImage` calls back into the host, which renders the layer's upstream effects
//! to produce the image, and one of those may be another effect of the *same* library. With a
//! plain mutex that render waits for a lock its own thread already holds and the app hangs.
//!
//! The lock is held by a *logical task* rather than an OS thread. Taking it gives the task a
//! token; a thread that holds a token may take the lock again (a nested render of an upstream
//! effect), and the threads that run the plug-in's work for that task adopt its tokens
//! ([`snapshot`], [`adopt`]): the pooled stack thread a render is moved onto, and the workers of the
//! plug-in's `multiThread` calls, whose `clipGetImage` calls nest just the same. Any other thread
//! waits its turn.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// The tokens of the locks the task running on this thread holds.
    static HELD: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

fn holds(token: u64) -> bool {
    HELD.with(|h| h.try_borrow().map(|h| h.contains(&token)).unwrap_or(false))
}

fn with_held(f: impl FnOnce(&mut Vec<u64>)) {
    HELD.with(|h| {
        if let Ok(mut h) = h.try_borrow_mut() {
            f(&mut h);
        }
    });
}

/// The identity of the task running on this thread: its oldest token (a task that holds no lock
/// has one only inside [`enter_task`]). `None` outside any task.
pub fn current_task() -> Option<u64> {
    HELD.with(|h| h.try_borrow().ok().and_then(|h| h.first().copied()))
}

/// Ends the task [`enter_task`] started when dropped.
pub struct Task(Option<u64>);

impl Drop for Task {
    fn drop(&mut self) {
        if let Some(token) = self.0.take() {
            with_held(|h| h.retain(|t| *t != token));
        }
    }
}

/// Give the work this thread starts an identity ([`current_task`]) if it has none yet: a render that
/// is the outermost of its task starts one, a nested render (on this thread, or on a thread working
/// for it) belongs to the task that is there. Tasks tell a render that waits for an instance
/// another task holds from one that would wait for itself.
pub fn enter_task() -> Task {
    if current_task().is_some() {
        return Task(None);
    }
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    with_held(|h| h.push(token));
    Task(Some(token))
}

/// The tokens the task on this thread holds, for the threads that work for it ([`adopt`]).
pub fn snapshot() -> Vec<u64> {
    HELD.with(|h| h.try_borrow().map(|h| h.clone()).unwrap_or_default())
}

/// Puts the thread's own tokens back when dropped.
pub struct Adopted(Option<Vec<u64>>);

impl Drop for Adopted {
    fn drop(&mut self) {
        if let Some(prev) = self.0.take() {
            with_held(|h| *h = prev);
        }
    }
}

/// Let the current thread act for the task that took `tokens` (from [`snapshot`] on the thread that
/// handed it work): it may take those locks again, until the guard is dropped.
pub fn adopt(tokens: &[u64]) -> Adopted {
    if tokens.is_empty() {
        return Adopted(None);
    }
    let mut prev = None;
    with_held(|h| {
        prev = Some(h.clone());
        for t in tokens {
            if !h.contains(t) {
                h.push(*t);
            }
        }
    });
    Adopted(prev)
}

#[derive(Default)]
struct State {
    /// The token of the task that holds the lock (0: free).
    owner: u64,
    /// How many times it holds it (nested renders, threads working for it).
    depth: u32,
}

/// The lock. See the module docs.
#[derive(Default)]
pub struct ReentrantLock {
    state: Mutex<State>,
    cv: Condvar,
}

impl ReentrantLock {
    pub fn new() -> ReentrantLock {
        ReentrantLock::default()
    }

    /// Take the lock, waiting while another task holds it. The task that holds it takes it again
    /// without waiting ([`Guard::nested`] tells).
    pub fn lock(&self) -> Guard<'_> {
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if st.owner == 0 {
                let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
                *st = State { owner: token, depth: 1 };
                with_held(|h| h.push(token));
                return Guard { lock: self, token, first: true, _not_send: PhantomData };
            }
            if holds(st.owner) {
                st.depth = st.depth.saturating_add(1);
                return Guard { lock: self, token: st.owner, first: false, _not_send: PhantomData };
            }
            st = self.cv.wait(st).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// Holds the lock until dropped.
pub struct Guard<'a> {
    lock: &'a ReentrantLock,
    token: u64,
    /// This guard took the lock (it was free), as opposed to a nested one.
    first: bool,
    /// Dropped on the thread that took it (the thread-local tokens belong to it).
    _not_send: PhantomData<*const ()>,
}

impl Guard<'_> {
    /// The task already held the lock: this is a nested use.
    pub fn nested(&self) -> bool {
        !self.first
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        let mut st = self.lock.state.lock().unwrap_or_else(PoisonError::into_inner);
        st.depth = st.depth.saturating_sub(1);
        if st.depth == 0 {
            st.owner = 0;
            self.lock.cv.notify_one();
        }
        drop(st);
        if self.first {
            let token = self.token;
            with_held(|h| h.retain(|t| *t != token));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    #[test]
    fn the_same_thread_may_take_the_lock_again() {
        let l = ReentrantLock::new();
        let a = l.lock();
        assert!(!a.nested());
        let b = l.lock();
        assert!(b.nested());
        drop(b);
        drop(a);
        // Free again, and a token never outlives its guard.
        assert!(snapshot().is_empty());
        let c = l.lock();
        assert!(!c.nested());
    }

    /// Another task waits until the holder is done.
    #[test]
    fn another_thread_waits_its_turn() {
        let l = Arc::new(ReentrantLock::new());
        let entered = Arc::new(AtomicBool::new(false));
        let guard = l.lock();
        let t = {
            let (l, entered) = (l.clone(), entered.clone());
            std::thread::spawn(move || {
                let _g = l.lock();
                entered.store(true, Ordering::SeqCst);
            })
        };
        std::thread::sleep(Duration::from_millis(100));
        assert!(!entered.load(Ordering::SeqCst), "it must not get in while the lock is held");
        drop(guard);
        t.join().unwrap();
        assert!(entered.load(Ordering::SeqCst));
    }

    /// The threads that work for the holder (a render moved to a pooled thread, the plug-in's
    /// `multiThread` workers) can take the lock again while the holder waits for them: with a
    /// plain mutex this is the deadlock.
    #[test]
    fn threads_working_for_the_holder_may_nest() {
        let l = Arc::new(ReentrantLock::new());
        let outer = l.lock();
        let tokens = snapshot();
        assert_eq!(tokens.len(), 1);
        let nested: Vec<bool> = std::thread::scope(|s| {
            let jobs: Vec<_> = (0..3)
                .map(|_| {
                    let (l, tokens) = (l.clone(), tokens.clone());
                    s.spawn(move || {
                        let _a = adopt(&tokens);
                        let g = l.lock();
                        let n = g.nested();
                        drop(g);
                        n
                    })
                })
                .collect();
            jobs.into_iter().map(|j| j.join().unwrap()).collect()
        });
        assert_eq!(nested, [true, true, true]);
        // A thread that did not adopt the tokens still has to wait: and does, until the holder is done.
        let l2 = l.clone();
        let stranger = std::thread::spawn(move || drop(l2.lock()));
        std::thread::sleep(Duration::from_millis(50));
        assert!(!stranger.is_finished());
        drop(outer);
        stranger.join().unwrap();
        // The workers' tokens went with their scopes.
        assert!(snapshot().is_empty());
    }

    #[test]
    fn a_task_is_known_to_the_threads_that_work_for_it() {
        assert_eq!(current_task(), None);
        let outer = enter_task();
        let id = current_task().expect("inside a task");
        // A nested task is the same task.
        {
            let _inner = enter_task();
            assert_eq!(current_task(), Some(id));
        }
        assert_eq!(current_task(), Some(id));
        // The lock a task takes does not change who it is, and a helper thread that adopts it is it.
        let l = ReentrantLock::new();
        let g = l.lock();
        assert_eq!(current_task(), Some(id));
        let tokens = snapshot();
        let seen = std::thread::spawn(move || {
            assert_eq!(current_task(), None);
            let _a = adopt(&tokens);
            current_task()
        })
        .join()
        .unwrap();
        assert_eq!(seen, Some(id));
        drop(g);
        drop(outer);
        assert_eq!(current_task(), None);
    }

    #[test]
    fn adopting_nothing_or_twice_changes_nothing() {
        let _none = adopt(&[]);
        assert!(snapshot().is_empty());
        let l = ReentrantLock::new();
        let g = l.lock();
        let tokens = snapshot();
        {
            let _again = adopt(&tokens);
            assert_eq!(snapshot(), tokens, "an owned token is not added twice");
        }
        assert_eq!(snapshot(), tokens);
        drop(g);
        assert!(snapshot().is_empty());
    }
}
