//! Terminal list management for Trigger All mode.
//!
//! Dynamic list of terminals (one per task). No fixed slots; only as many
//! entries as needed. A limited number run concurrently; when one finishes,
//! the next from the queue starts.

use std::fmt;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use portable_pty::MasterPty;
use uuid::Uuid;

/// Maximum number of terminals running at the same time (to avoid too many PTYs)
pub const MAX_CONCURRENT_PTYS: usize = 4;

/// One entry in the terminal list: a task and its running terminal (if any)
#[derive(Debug)]
pub struct TerminalEntry {
    pub task_id: Uuid,
    pub slot: Option<TerminalSlot>,
}

/// State for a single terminal (PTY + parser) while a task is running
pub struct TerminalSlot {
    /// Task ID being executed in this slot
    pub task_id: Uuid,
    /// VT100 parser for terminal emulation
    pub parser: Arc<RwLock<vt100::Parser>>,
    /// Writer to send input to the PTY
    pub writer: Option<Box<dyn Write + Send>>,
    /// Master PTY handle for resizing
    pub master: Option<Box<dyn MasterPty + Send>>,
    /// Flag indicating if PTY process is still running
    pub running: Arc<Mutex<bool>>,
    /// Exit code when process has finished (None = still running or unknown)
    pub exit_status: Arc<Mutex<Option<i32>>>,
    /// Slot size (rows, cols) for resize
    pub size: (u16, u16),
}

impl fmt::Debug for TerminalSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TerminalSlot")
            .field("task_id", &self.task_id)
            .field("size", &self.size)
            .field("has_writer", &self.writer.is_some())
            .field("has_master", &self.master.is_some())
            .finish()
    }
}

impl TerminalSlot {
    pub fn new(task_id: Uuid, rows: u16, cols: u16) -> Self {
        Self {
            task_id,
            parser: Arc::new(RwLock::new(vt100::Parser::new(rows, cols, 1000))),
            writer: None,
            master: None,
            running: Arc::new(Mutex::new(true)),
            exit_status: Arc::new(Mutex::new(None)),
            size: (rows, cols),
        }
    }

    pub fn is_running(&self) -> bool {
        *self
            .running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Exit code: None = still running, Some(0) = success, Some(n) = failed with code n
    pub fn exit_code(&self) -> Option<i32> {
        *self
            .exit_status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Context needed to spawn new tasks from the queue
#[derive(Clone, Debug)]
pub struct TriggerAllContext {
    pub workflow_path: PathBuf,
    pub run_id: Uuid,
    pub target_path: Option<PathBuf>,
}

/// Manages the task queue and active terminal slots for Trigger All mode
#[derive(Debug, Default)]
pub struct TerminalQueueState {
    /// Tasks waiting to be executed
    pub pending_queue: Vec<Uuid>,
    /// Context for spawning (workflow path, run_id, etc.)
    pub context: Option<TriggerAllContext>,
    /// When trigger-all was started (for status display)
    pub started_at: Option<Instant>,
}

impl TerminalQueueState {
    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        self.context.is_some()
    }

    pub fn pending_count(&self) -> usize {
        self.pending_queue.len()
    }

    /// Pop the next task from the queue, if any
    pub fn pop_next(&mut self) -> Option<Uuid> {
        if self.pending_queue.is_empty() {
            None
        } else {
            Some(self.pending_queue.remove(0))
        }
    }

    /// Clear and deactivate the queue
    pub fn clear(&mut self) {
        self.pending_queue.clear();
        self.context = None;
        self.started_at = None;
    }
}
