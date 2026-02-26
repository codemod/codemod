use chrono::Utc;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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
#[derive(Clone)]
pub struct StructuredLogger {
    format: OutputFormat,
    seq: Arc<AtomicU64>,
    context: Option<StepContext>,
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
        }
    }

    /// Return a clone with the given step context set.
    /// Shares the same `seq` counter with the parent.
    pub fn with_context(&self, ctx: StepContext) -> Self {
        Self {
            format: self.format,
            seq: Arc::clone(&self.seq),
            context: Some(ctx),
        }
    }

    /// Check if JSONL mode is active.
    pub fn is_jsonl(&self) -> bool {
        self.format == OutputFormat::Jsonl
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
            println!("{json}");
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
            println!("{json}");
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
            println!("{json}");
        }
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
