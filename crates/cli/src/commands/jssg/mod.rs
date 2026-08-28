pub mod bundle;
pub mod config;
pub mod exec;
pub mod list_applicable;
pub mod run;
pub mod test;

use codemod_sandbox::sandbox::runtime_module::{
    RuntimeEvent, RuntimeEventCallback, RuntimeEventKind,
};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
struct RuntimeEventBuffer {
    entries: Arc<Mutex<BufferedRuntimeEvents>>,
}

#[derive(Default)]
struct BufferedRuntimeEvents {
    order: Vec<String>,
    entries: HashMap<String, Vec<String>>,
}

#[derive(Clone)]
struct RuntimeEventOutput {
    stream: RuntimeEventOutputStream,
    lock: Arc<Mutex<()>>,
}

#[derive(Clone, Copy)]
enum RuntimeEventOutputStream {
    Stdout,
    Stderr,
}

impl RuntimeEventOutput {
    fn new(stream: RuntimeEventOutputStream) -> Self {
        Self {
            stream,
            lock: Arc::new(Mutex::new(())),
        }
    }

    fn stdout() -> Self {
        Self::new(RuntimeEventOutputStream::Stdout)
    }

    fn stderr() -> Self {
        Self::new(RuntimeEventOutputStream::Stderr)
    }

    fn flush(&self, buffer: &RuntimeEventBuffer) {
        let groups = buffer.drain();
        if groups.is_empty() {
            return;
        }

        let Ok(_guard) = self.lock.lock() else {
            return;
        };
        for (title, lines) in groups {
            self.write("");
            self.write(&title);
            for line in lines {
                for line in line.lines() {
                    self.write(&format!("  {line}"));
                }
            }
        }
    }

    fn write(&self, line: &str) {
        match self.stream {
            RuntimeEventOutputStream::Stdout => println!("{line}"),
            RuntimeEventOutputStream::Stderr => eprintln!("{line}"),
        }
    }
}

impl RuntimeEventBuffer {
    fn new() -> Self {
        Self::default()
    }

    fn callback_for_title(&self, title: impl Into<String>) -> RuntimeEventCallback {
        let title = title.into();
        let entries = Arc::clone(&self.entries);

        Arc::new(move |event| {
            let Some(message) = format_runtime_event_log(&event) else {
                return;
            };
            if message.trim().is_empty() {
                return;
            }

            let Ok(mut buffer) = entries.lock() else {
                return;
            };
            if !buffer.entries.contains_key(&title) {
                buffer.order.push(title.clone());
            }
            buffer
                .entries
                .entry(title.clone())
                .or_default()
                .push(message);
        })
    }

    fn drain(&self) -> Vec<(String, Vec<String>)> {
        let buffered = self
            .entries
            .lock()
            .map(|mut buffer| std::mem::take(&mut *buffer))
            .unwrap_or_default();

        buffered
            .order
            .into_iter()
            .filter_map(|title| {
                buffered
                    .entries
                    .get(&title)
                    .map(|lines| (title, lines.clone()))
            })
            .collect()
    }
}

fn display_path_title(path: &Path, base: Option<&Path>) -> String {
    base.and_then(|base| path.strip_prefix(base).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

fn format_runtime_event_log(event: &RuntimeEvent) -> Option<String> {
    if event.meta.as_deref() == Some("console") {
        return Some(event.message.clone());
    }

    let prefix = match event.kind {
        RuntimeEventKind::Progress => "[progress]",
        RuntimeEventKind::Warn => "[warn]",
        RuntimeEventKind::SetCurrentUnit => return None,
    };

    let mut message = format!("{prefix} {}", event.message);
    if let Some(meta) = &event.meta {
        message.push(' ');
        message.push_str(meta);
    }
    Some(message)
}

#[cfg(test)]
mod tests {
    use codemod_sandbox::sandbox::runtime_module::{RuntimeEvent, RuntimeEventKind};
    use std::path::Path;

    #[test]
    fn formats_console_events_without_runtime_prefix() {
        let event = RuntimeEvent {
            kind: RuntimeEventKind::Progress,
            message: "hello from transform".to_string(),
            meta: Some("console".to_string()),
        };

        assert_eq!(
            super::format_runtime_event_log(&event),
            Some("hello from transform".to_string())
        );
    }

    #[test]
    fn formats_runtime_warnings_with_prefix() {
        let event = RuntimeEvent {
            kind: RuntimeEventKind::Warn,
            message: "missing optional data".to_string(),
            meta: None,
        };

        assert_eq!(
            super::format_runtime_event_log(&event),
            Some("[warn] missing optional data".to_string())
        );
    }

    #[test]
    fn skips_current_unit_events() {
        let event = RuntimeEvent {
            kind: RuntimeEventKind::SetCurrentUnit,
            message: "phase 1".to_string(),
            meta: None,
        };

        assert_eq!(super::format_runtime_event_log(&event), None);
    }

    #[test]
    fn buffers_runtime_events_by_title_in_first_seen_order() {
        let buffer = super::RuntimeEventBuffer::new();
        let first = buffer.callback_for_title("fixtures/a.js");
        let second = buffer.callback_for_title("fixtures/b.js");

        first(RuntimeEvent {
            kind: RuntimeEventKind::Progress,
            message: "a1".to_string(),
            meta: Some("console".to_string()),
        });
        second(RuntimeEvent {
            kind: RuntimeEventKind::Warn,
            message: "b1".to_string(),
            meta: None,
        });
        first(RuntimeEvent {
            kind: RuntimeEventKind::Progress,
            message: "a2".to_string(),
            meta: Some("console".to_string()),
        });

        assert_eq!(
            buffer.drain(),
            vec![
                (
                    "fixtures/a.js".to_string(),
                    vec!["a1".to_string(), "a2".to_string()]
                ),
                ("fixtures/b.js".to_string(), vec!["[warn] b1".to_string()])
            ]
        );
    }

    #[test]
    fn draining_runtime_event_buffer_clears_entries() {
        let buffer = super::RuntimeEventBuffer::new();
        let callback = buffer.callback_for_title("fixtures/a.js");

        callback(RuntimeEvent {
            kind: RuntimeEventKind::Progress,
            message: "a1".to_string(),
            meta: Some("console".to_string()),
        });

        assert_eq!(buffer.drain().len(), 1);
        assert!(buffer.drain().is_empty());
    }

    #[test]
    fn display_path_title_prefers_relative_path_inside_base() {
        assert_eq!(
            super::display_path_title(
                Path::new("/tmp/project/src/a.js"),
                Some(Path::new("/tmp/project"))
            ),
            "src/a.js"
        );
        assert_eq!(
            super::display_path_title(
                Path::new("/tmp/other/a.js"),
                Some(Path::new("/tmp/project"))
            ),
            "/tmp/other/a.js"
        );
    }
}
