//! Workspace tables built once, however many threads ask at the same time.
//!
//! A step's `forEachFile` workers share one provider. The first worker to
//! ask for a table (the callers table, the importers table) builds it; the
//! others wait for that build instead of starting their own.
//!
//! The build runs on a thread pool of its own, from a thread of its own.
//! A waiting thread blocks without taking work from any pool, and the build
//! never needs a thread that is waiting for it, so the two cannot deadlock
//! however the callers are scheduled (plain threads, rayon jobs, nested).

use parking_lot::{Condvar, Mutex};
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, OnceLock};

enum State<T> {
    Empty,
    Building,
    Built(Arc<T>),
}

/// A value built once, on demand, and kept until [`BuildOnce::clear`].
pub struct BuildOnce<T> {
    state: Mutex<State<T>>,
    ready: Condvar,
}

impl<T> Default for BuildOnce<T> {
    fn default() -> Self {
        BuildOnce {
            state: Mutex::new(State::Empty),
            ready: Condvar::new(),
        }
    }
}

impl<T: Send + Sync> BuildOnce<T> {
    /// The value, built by `build` if no one has built it yet. While another
    /// thread builds it, wait for that build. If a build panics, the panic
    /// reaches its caller and the next caller builds again.
    pub fn get_or_build(&self, build: impl FnOnce() -> T + Send) -> Arc<T> {
        {
            let mut state = self.state.lock();
            loop {
                match &*state {
                    State::Built(value) => return Arc::clone(value),
                    State::Building => self.ready.wait(&mut state),
                    State::Empty => {
                        *state = State::Building;
                        break;
                    }
                }
            }
        }
        let built = std::panic::catch_unwind(AssertUnwindSafe(|| on_build_pool(build)));
        let mut state = self.state.lock();
        match built {
            Ok(value) => {
                let value = Arc::new(value);
                *state = State::Built(Arc::clone(&value));
                self.ready.notify_all();
                value
            }
            Err(panic) => {
                *state = State::Empty;
                self.ready.notify_all();
                drop(state);
                std::panic::resume_unwind(panic)
            }
        }
    }

    /// The value, if it is built.
    pub fn get(&self) -> Option<Arc<T>> {
        match &*self.state.lock() {
            State::Built(value) => Some(Arc::clone(value)),
            _ => None,
        }
    }

    /// Drop the value; the next caller builds it again. A build already
    /// running still stores its result when it finishes.
    pub fn clear(&self) {
        let mut state = self.state.lock();
        if matches!(*state, State::Built(_)) {
            *state = State::Empty;
        }
    }
}

/// The pool workspace tables are built on, apart from rayon's global pool.
fn build_pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .thread_name(|index| format!("codemod-semantic-{index}"))
            .build()
            .expect("semantic analysis thread pool")
    })
}

/// Run `work` on the build pool, from a fresh thread: the calling thread
/// only waits for it (a worker of another pool would otherwise run that
/// pool's jobs while it waits, and one of them could be waiting for us).
fn on_build_pool<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        scope
            .spawn(|| build_pool().install(work))
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn concurrent_askers_share_one_build() {
        let once: BuildOnce<usize> = BuildOnce::default();
        let builds = AtomicUsize::new(0);
        let values: Vec<usize> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        *once.get_or_build(|| {
                            builds.fetch_add(1, Ordering::SeqCst);
                            std::thread::sleep(Duration::from_millis(50));
                            42
                        })
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(values, vec![42; 8]);
        assert_eq!(builds.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn askers_inside_rayon_jobs_do_not_deadlock() {
        use rayon::prelude::*;
        // Every global-pool thread asks while the build itself needs a pool.
        let once: BuildOnce<usize> = BuildOnce::default();
        let total: usize = (0..64usize)
            .into_par_iter()
            .map(|_| *once.get_or_build(|| (0..1000usize).into_par_iter().sum()))
            .sum();
        assert_eq!(total, 64 * 499_500);
    }

    #[test]
    fn a_failed_build_is_retried() {
        let once: BuildOnce<usize> = BuildOnce::default();
        let failed = std::panic::catch_unwind(AssertUnwindSafe(|| {
            once.get_or_build(|| panic!("no"));
        }));
        assert!(failed.is_err());
        assert_eq!(*once.get_or_build(|| 7), 7);
        once.clear();
        assert_eq!(*once.get_or_build(|| 8), 8);
    }
}
