//! Cooperative cancellation (`docs/PLAN.md` 5.4).
//!
//! `ABOR`, the run watchdog and process shutdown all go through one mechanism: an atomic flag
//! plus a condition variable for waiters, plus a list of hooks so a blocking socket read can be
//! unblocked with `TcpStream::shutdown`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

type Hook = Box<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Inner {
    flag: AtomicBool,
    /// Guards the hook list and pairs with `changed`; the boolean mirrors `flag` so waiters
    /// have something to predicate on.
    state: Mutex<CancelState>,
    changed: Condvar,
}

#[derive(Default)]
struct CancelState {
    cancelled: bool,
    hooks: Vec<Hook>,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelToken")
            .field("cancelled", &self.flag.load(Ordering::Acquire))
            .finish()
    }
}

/// A clonable cancellation handle. Cloning shares one underlying flag.
#[derive(Clone, Debug, Default)]
pub struct CancelToken {
    inner: Arc<Inner>,
}

impl CancelToken {
    /// A fresh, un-cancelled token.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.flag.load(Ordering::Acquire)
    }

    /// Request cancellation, wake every waiter and run every registered hook exactly once.
    ///
    /// Idempotent: a second call does nothing.
    pub fn cancel(&self) {
        let hooks = {
            let mut guard = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if guard.cancelled {
                return;
            }
            guard.cancelled = true;
            self.inner.flag.store(true, Ordering::Release);
            std::mem::take(&mut guard.hooks)
        };
        self.inner.changed.notify_all();
        for hook in hooks {
            hook();
        }
    }

    /// Register a callback to run when the token is cancelled.
    ///
    /// If the token is already cancelled the hook runs immediately on the calling thread. This
    /// is how a reader parked in a blocking `read` is woken: the hook calls
    /// `TcpStream::shutdown` on a cloned handle.
    pub fn on_cancel(&self, hook: impl Fn() + Send + Sync + 'static) {
        let mut guard = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if guard.cancelled {
            drop(guard);
            hook();
            return;
        }
        guard.hooks.push(Box::new(hook));
    }

    /// Block until cancelled or until `timeout` elapses. Returns `true` if cancelled.
    #[must_use]
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        let guard = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (guard, _) = self
            .inner
            .changed
            .wait_timeout_while(guard, timeout, |s| !s.cancelled)
            .unwrap_or_else(PoisonError::into_inner);
        guard.cancelled
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn starts_uncancelled() {
        assert!(!CancelToken::new().is_cancelled());
    }

    #[test]
    fn cancel_is_visible_through_clones() {
        let a = CancelToken::new();
        let b = a.clone();
        a.cancel();
        assert!(b.is_cancelled());
    }

    #[test]
    fn hooks_run_once_on_cancel() {
        let count = Arc::new(AtomicUsize::new(0));
        let token = CancelToken::new();
        let c = Arc::clone(&count);
        token.on_cancel(move || {
            c.fetch_add(1, Ordering::SeqCst);
        });
        token.cancel();
        token.cancel();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn hook_registered_after_cancel_runs_immediately() {
        let count = Arc::new(AtomicUsize::new(0));
        let token = CancelToken::new();
        token.cancel();
        let c = Arc::clone(&count);
        token.on_cancel(move || {
            c.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn wait_timeout_returns_false_when_not_cancelled() {
        let token = CancelToken::new();
        assert!(!token.wait_timeout(Duration::from_millis(5)));
    }

    #[test]
    fn wait_timeout_wakes_on_cancel() {
        let token = CancelToken::new();
        let other = token.clone();
        let handle = std::thread::spawn(move || other.wait_timeout(Duration::from_secs(30)));
        // Cancel from this thread; the waiter must return promptly with `true`.
        std::thread::sleep(Duration::from_millis(10));
        token.cancel();
        assert!(handle.join().unwrap());
    }
}
