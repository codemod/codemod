use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyEvent};
use futures::StreamExt;
use tokio::sync::mpsc;

/// Events that the TUI can receive
#[derive(Debug)]
#[allow(dead_code)]
pub enum AppEvent {
    /// A key was pressed
    Key(KeyEvent),
    /// A tick interval elapsed (for refreshing data)
    Tick,
    /// Terminal was resized
    Resize(u16, u16),
}

/// Async event handler that merges crossterm events with a tick interval
pub struct EventHandler {
    rx: mpsc::UnboundedReceiver<AppEvent>,
    _task: tokio::task::JoinHandle<()>,
}

impl EventHandler {
    /// Create a new event handler with the given tick rate
    pub fn new(tick_rate: std::time::Duration) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

        let task = tokio::spawn(async move {
            let mut reader = EventStream::new();
            let mut tick_interval = tokio::time::interval(tick_rate);

            loop {
                tokio::select! {
                    _ = tick_interval.tick() => {
                        if tx.send(AppEvent::Tick).is_err() {
                            break;
                        }
                    }
                    Some(Ok(event)) = reader.next() => {
                        match event {
                            Event::Key(key) => {
                                if tx.send(AppEvent::Key(key)).is_err() {
                                    break;
                                }
                            }
                            Event::Resize(w, h) => {
                                if tx.send(AppEvent::Resize(w, h)).is_err() {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        });

        Self { rx, _task: task }
    }

    /// Wait for the next event
    pub async fn next(&mut self) -> Result<AppEvent> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("Event channel closed"))
    }
}

impl Drop for EventHandler {
    fn drop(&mut self) {
        // Abort the background reader task so it stops consuming terminal
        // events. Without this, dropped handlers (e.g. from the log viewer)
        // leave zombie tasks that steal events from the main event loop.
        self._task.abort();
    }
}
