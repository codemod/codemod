use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyEvent, MouseEvent};
use futures::StreamExt;
use tokio::sync::mpsc;

/// Events that the TUI can receive
#[derive(Debug)]
#[allow(dead_code)]
pub enum AppEvent {
    /// A key was pressed
    Key(KeyEvent),
    /// A mouse event was received
    Mouse(MouseEvent),
    /// A coalesced scroll delta from one or more mouse events
    Scroll(i32),
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
                    result = reader.next() => {
                        match result {
                            Some(Ok(event)) => match event {
                                Event::Key(key) => {
                                    if tx.send(AppEvent::Key(key)).is_err() {
                                        break;
                                    }
                                }
                                Event::Mouse(mouse) => {
                                    if tx.send(AppEvent::Mouse(mouse)).is_err() {
                                        break;
                                    }
                                }
                                Event::Resize(w, h) => {
                                    if tx.send(AppEvent::Resize(w, h)).is_err() {
                                        break;
                                    }
                                }
                                _ => {}
                            },
                            _ => break,
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

    /// Drain any currently queued events without waiting.
    pub fn drain_pending(&mut self, limit: usize) -> Vec<AppEvent> {
        let mut events = Vec::new();
        while events.len() < limit {
            match self.rx.try_recv() {
                Ok(event) => events.push(event),
                Err(_) => break,
            }
        }
        events
    }
}

impl Drop for EventHandler {
    fn drop(&mut self) {
        self._task.abort();
    }
}
