use chrono::Utc;
use serde::Serialize;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Output format for the engine
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Text,
    Jsonl,
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "text" => Ok(OutputFormat::Text),
            "jsonl" => Ok(OutputFormat::Jsonl),
            other => Err(format!("unknown output format: {other}")),
        }
    }
}

/// Context identifying the current step being executed
#[derive(Debug, Clone)]
pub struct StepContext {
    pub step_name: String,
    pub step_index: usize,
    pub node_id: String,
    pub node_name: String,
    pub task_id: String,
}

/// A JSONL log record emitted to stdout
#[derive(Serialize)]
struct JsonlRecord {
    seq: u64,
    ts: String,
    level: String,
    event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    msg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    step_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    step_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
}

/// Structured logger that emits JSONL lines to stdout when in Jsonl mode.
///
/// The `seq` counter is shared (via `Arc<AtomicU64>`) across all clones,
/// ensuring globally ordered sequence numbers even from rayon threads.
///
/// The `override_fd` is shared across all clones. When set by `StdoutCaptureGuard`,
/// output is written directly to the saved fd (bypassing any fd 1 redirect).
#[derive(Clone)]
pub struct StructuredLogger {
    format: OutputFormat,
    seq: Arc<AtomicU64>,
    context: Option<StepContext>,
    /// When set, JSONL output is written to this raw fd instead of stdout.
    /// Used by `StdoutCaptureGuard` to bypass fd 1 redirects.
    override_fd: Arc<Mutex<Option<i32>>>,
}

impl Default for StructuredLogger {
    fn default() -> Self {
        Self::new(OutputFormat::Text)
    }
}

impl StructuredLogger {
    /// Create a new logger with the given output format.
    pub fn new(format: OutputFormat) -> Self {
        Self {
            format,
            seq: Arc::new(AtomicU64::new(0)),
            context: None,
            override_fd: Arc::new(Mutex::new(None)),
        }
    }

    /// Return a clone with the given step context set.
    /// Shares the same `seq` counter and `override_fd` with the parent.
    pub fn with_context(&self, ctx: StepContext) -> Self {
        Self {
            format: self.format,
            seq: Arc::clone(&self.seq),
            context: Some(ctx),
            override_fd: Arc::clone(&self.override_fd),
        }
    }

    /// Check if JSONL mode is active.
    pub fn is_jsonl(&self) -> bool {
        self.format == OutputFormat::Jsonl
    }

    /// Write a line to the output target. When `override_fd` is set (during
    /// stdout capture), writes directly to the saved fd. Otherwise writes to stdout.
    fn write_output(&self, line: &str) {
        #[cfg(unix)]
        {
            let guard = self.override_fd.lock().unwrap();
            if let Some(fd) = *guard {
                let data = format!("{line}\n");
                let bytes = data.as_bytes();
                unsafe {
                    libc::write(fd, bytes.as_ptr() as *const libc::c_void, bytes.len());
                }
                return;
            }
        }
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }

    /// Emit a log event. In JSONL mode, writes a JSON line to stdout.
    /// In Text mode, this is a no-op (caller should use `log!` macros).
    pub fn log(&self, level: &str, msg: &str) {
        if self.format != OutputFormat::Jsonl {
            return;
        }
        let record = JsonlRecord {
            seq: self.seq.fetch_add(1, Ordering::Relaxed),
            ts: Utc::now().to_rfc3339(),
            level: level.to_string(),
            event: "log".to_string(),
            msg: Some(msg.to_string()),
            step_name: self.context.as_ref().map(|c| c.step_name.clone()),
            step_index: self.context.as_ref().map(|c| c.step_index),
            node_id: self.context.as_ref().map(|c| c.node_id.clone()),
            node_name: self.context.as_ref().map(|c| c.node_name.clone()),
            task_id: self.context.as_ref().map(|c| c.task_id.clone()),
            outcome: None,
            duration_ms: None,
        };
        if let Ok(json) = serde_json::to_string(&record) {
            self.write_output(&json);
        }
    }

    /// Emit a step_start event.
    pub fn step_start(&self) {
        if self.format != OutputFormat::Jsonl {
            return;
        }
        let ctx = match &self.context {
            Some(c) => c,
            None => return,
        };
        let record = JsonlRecord {
            seq: self.seq.fetch_add(1, Ordering::Relaxed),
            ts: Utc::now().to_rfc3339(),
            level: "info".to_string(),
            event: "step_start".to_string(),
            msg: None,
            step_name: Some(ctx.step_name.clone()),
            step_index: Some(ctx.step_index),
            node_id: Some(ctx.node_id.clone()),
            node_name: Some(ctx.node_name.clone()),
            task_id: Some(ctx.task_id.clone()),
            outcome: None,
            duration_ms: None,
        };
        if let Ok(json) = serde_json::to_string(&record) {
            self.write_output(&json);
        }
    }

    /// Emit a step_end event.
    pub fn step_end(&self, outcome: &str, duration_ms: u64) {
        if self.format != OutputFormat::Jsonl {
            return;
        }
        let ctx = match &self.context {
            Some(c) => c,
            None => return,
        };
        let record = JsonlRecord {
            seq: self.seq.fetch_add(1, Ordering::Relaxed),
            ts: Utc::now().to_rfc3339(),
            level: "info".to_string(),
            event: "step_end".to_string(),
            msg: None,
            step_name: Some(ctx.step_name.clone()),
            step_index: Some(ctx.step_index),
            node_id: Some(ctx.node_id.clone()),
            node_name: Some(ctx.node_name.clone()),
            task_id: Some(ctx.task_id.clone()),
            outcome: Some(outcome.to_string()),
            duration_ms: Some(duration_ms),
        };
        if let Ok(json) = serde_json::to_string(&record) {
            self.write_output(&json);
        }
    }
}

// ---------------------------------------------------------------------------
// Stdout capture guard — redirects fd 1 to a pipe during its lifetime
// ---------------------------------------------------------------------------

/// RAII guard that captures all stdout (fd 1) writes during its lifetime.
///
/// When active, fd 1 is redirected to a pipe. A background thread reads from
/// the pipe and wraps each captured line in JSONL via the step logger, writing
/// directly to the saved real stdout. The structured logger itself also writes
/// to the saved real stdout (via `override_fd`), so its output is not captured.
///
/// On drop, the original stdout is restored and the reader thread is joined.
pub struct StdoutCaptureGuard {
    #[cfg(unix)]
    saved_fd: i32,
    #[cfg(unix)]
    reader_handle: Option<std::thread::JoinHandle<()>>,
    #[cfg(unix)]
    override_fd: Arc<Mutex<Option<i32>>>,
}

impl StdoutCaptureGuard {
    /// Start capturing stdout. Returns `Some(guard)` on success, `None` on failure
    /// or on non-Unix platforms.
    #[cfg(unix)]
    pub fn start(logger: &StructuredLogger) -> Option<Self> {
        use std::os::unix::io::FromRawFd;

        // Save current stdout fd
        let saved_fd = unsafe { libc::dup(1) };
        if saved_fd == -1 {
            return None;
        }

        // Create pipe
        let mut pipe_fds = [0i32; 2];
        if unsafe { libc::pipe(pipe_fds.as_mut_ptr()) } == -1 {
            unsafe { libc::close(saved_fd) };
            return None;
        }
        let read_fd = pipe_fds[0];
        let write_fd = pipe_fds[1];

        // Redirect fd 1 to the pipe write end
        if unsafe { libc::dup2(write_fd, 1) } == -1 {
            unsafe {
                libc::close(saved_fd);
                libc::close(read_fd);
                libc::close(write_fd);
            }
            return None;
        }
        // Close the original write_fd; fd 1 is now the only write end
        unsafe { libc::close(write_fd) };

        // Tell the logger to write to the saved fd (bypassing fd 1 redirect)
        let override_fd = Arc::clone(&logger.override_fd);
        *override_fd.lock().unwrap() = Some(saved_fd);

        // Spawn reader thread: reads captured stdout lines and wraps in JSONL
        let cb_logger = logger.clone();
        let handle = std::thread::spawn(move || {
            let read_file = unsafe { std::fs::File::from_raw_fd(read_fd) };
            let reader = std::io::BufReader::new(read_file);
            use std::io::BufRead;
            for line in reader.lines() {
                match line {
                    Ok(line) if !line.is_empty() => {
                        cb_logger.log("info", &line);
                    }
                    Ok(_) => {} // skip empty lines
                    Err(_) => break,
                }
            }
            // read_file is dropped here, closing read_fd
        });

        Some(Self {
            saved_fd,
            reader_handle: Some(handle),
            override_fd,
        })
    }

    /// Non-Unix fallback: capture is not supported.
    #[cfg(not(unix))]
    pub fn start(_logger: &StructuredLogger) -> Option<Self> {
        None
    }
}

#[cfg(unix)]
impl Drop for StdoutCaptureGuard {
    fn drop(&mut self) {
        // Restore original stdout. dup2 atomically closes the old fd 1 (pipe
        // write end), which causes the reader thread to see EOF.
        unsafe { libc::dup2(self.saved_fd, 1) };

        // Wait for the reader thread to drain remaining data and finish.
        // This is a brief blocking wait since the pipe write end is closed.
        if let Some(handle) = self.reader_handle.take() {
            handle.join().ok();
        }

        // Reset override_fd so the logger goes back to normal stdout
        *self.override_fd.lock().unwrap() = None;

        // Close the saved fd (fd 1 still works — dup2 created a new reference)
        unsafe { libc::close(self.saved_fd) };
    }
}

/// Dual-mode logging macro. In JSONL mode, emits via the structured logger.
/// In Text mode, falls through to the `log` crate macros.
#[macro_export]
macro_rules! slog {
    ($logger:expr, $level:ident, $($arg:tt)*) => {
        if $logger.is_jsonl() {
            $logger.log(stringify!($level), &format!($($arg)*));
        } else {
            log::$level!($($arg)*);
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_format_parse() {
        assert_eq!("text".parse::<OutputFormat>().unwrap(), OutputFormat::Text);
        assert_eq!(
            "jsonl".parse::<OutputFormat>().unwrap(),
            OutputFormat::Jsonl
        );
        assert_eq!(
            "JSONL".parse::<OutputFormat>().unwrap(),
            OutputFormat::Jsonl
        );
        assert!("xml".parse::<OutputFormat>().is_err());
    }

    #[test]
    fn test_seq_counter_shared_across_clones() {
        let logger = StructuredLogger::new(OutputFormat::Jsonl);
        let ctx = StepContext {
            step_name: "test".to_string(),
            step_index: 0,
            node_id: "n1".to_string(),
            node_name: "Node 1".to_string(),
            task_id: "t1".to_string(),
        };
        let child = logger.with_context(ctx);
        // Both share the same seq counter
        assert_eq!(logger.seq.load(Ordering::Relaxed), 0);
        child.log("info", "hello");
        assert_eq!(logger.seq.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_text_mode_is_noop() {
        let logger = StructuredLogger::new(OutputFormat::Text);
        assert!(!logger.is_jsonl());
        // These should not panic or produce output
        logger.log("info", "hello");
        logger.step_start();
        logger.step_end("success", 100);
    }
}
