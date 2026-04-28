//! Periodic-tick helper used by step-scoped watchdogs (idle detection,
//! cancellation polling) to keep that kind of work off any hot loop.

use std::future::Future;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;

/// Spawn a tokio task that runs `tick` every `period` until either:
/// - `done` is flipped to `true` by the owner at teardown, or
/// - `tick` returns `ControlFlow::Break(())` — the watchdog "fired."
///
/// `done` is checked both before and after each sleep so the task exits
/// promptly when the owning step completes. The helper is intentionally
/// minimal: side effects (setting flags, notifying waiters, publishing
/// events) live inside `tick` so each caller can fire however it needs.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn spawn_periodic<F, Fut>(
    period: Duration,
    done: Arc<AtomicBool>,
    mut tick: F,
) -> JoinHandle<()>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = ControlFlow<()>> + Send + 'static,
{
    tokio::spawn(async move {
        loop {
            if done.load(Ordering::Acquire) {
                return;
            }
            tokio::time::sleep(period).await;
            if done.load(Ordering::Acquire) {
                return;
            }
            if tick().await.is_break() {
                return;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[tokio::test]
    async fn stops_when_done_is_set() {
        let done = Arc::new(AtomicBool::new(false));
        let ticks = Arc::new(AtomicUsize::new(0));
        let ticks_for_closure = Arc::clone(&ticks);
        let handle = spawn_periodic(Duration::from_millis(5), Arc::clone(&done), move || {
            let ticks = Arc::clone(&ticks_for_closure);
            async move {
                ticks.fetch_add(1, Ordering::Relaxed);
                ControlFlow::Continue(())
            }
        });

        tokio::time::sleep(Duration::from_millis(30)).await;
        done.store(true, Ordering::Release);
        handle.await.unwrap();
        assert!(ticks.load(Ordering::Relaxed) >= 1);
    }

    #[tokio::test]
    async fn stops_when_tick_breaks() {
        let done = Arc::new(AtomicBool::new(false));
        let ticks = Arc::new(AtomicUsize::new(0));
        let ticks_for_closure = Arc::clone(&ticks);
        let handle = spawn_periodic(Duration::from_millis(5), done, move || {
            let ticks = Arc::clone(&ticks_for_closure);
            async move {
                let n = ticks.fetch_add(1, Ordering::Relaxed) + 1;
                if n >= 3 {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            }
        });

        handle.await.unwrap();
        assert_eq!(ticks.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn never_ticks_when_done_is_preset() {
        let done = Arc::new(AtomicBool::new(true));
        let ticks = Arc::new(AtomicUsize::new(0));
        let ticks_for_closure = Arc::clone(&ticks);
        let handle = spawn_periodic(Duration::from_millis(5), done, move || {
            let ticks = Arc::clone(&ticks_for_closure);
            async move {
                ticks.fetch_add(1, Ordering::Relaxed);
                ControlFlow::Continue(())
            }
        });

        handle.await.unwrap();
        assert_eq!(ticks.load(Ordering::Relaxed), 0);
    }
}
