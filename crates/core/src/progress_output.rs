use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::execution::ProgressCallback;

#[derive(Default)]
pub(crate) struct BufferedExecutionOutput {
    order: Vec<String>,
    entries: HashMap<String, BufferedExecutionOutputEntry>,
}

#[derive(Default)]
struct BufferedExecutionOutputEntry {
    logs: Vec<String>,
    diagnostics: Vec<String>,
}

pub(crate) type SharedBufferedExecutionOutput = Arc<Mutex<BufferedExecutionOutput>>;

pub(crate) fn append_buffered_log(
    buffer: &SharedBufferedExecutionOutput,
    title: String,
    line: String,
) {
    if line.trim().is_empty() {
        return;
    }

    let Ok(mut buffer) = buffer.lock() else {
        return;
    };
    if !buffer.entries.contains_key(&title) {
        buffer.order.push(title.clone());
    }
    buffer.entries.entry(title).or_default().logs.push(line);
}

pub(crate) fn append_buffered_diagnostic(
    buffer: &SharedBufferedExecutionOutput,
    title: String,
    diagnostic: String,
) {
    if diagnostic.trim().is_empty() {
        return;
    }

    let Ok(mut buffer) = buffer.lock() else {
        return;
    };
    if !buffer.entries.contains_key(&title) {
        buffer.order.push(title.clone());
    }
    buffer
        .entries
        .entry(title)
        .or_default()
        .diagnostics
        .push(diagnostic);
}

pub(crate) fn flush_buffered_execution_output(
    buffer: &SharedBufferedExecutionOutput,
    progress_callback: &Arc<Option<ProgressCallback>>,
    task_id: &str,
) {
    let Some(callback) = progress_callback.as_ref() else {
        return;
    };

    let buffered = buffer
        .lock()
        .map(|mut buffer| std::mem::take(&mut *buffer))
        .unwrap_or_default();

    for title in buffered.order {
        let Some(entry) = buffered.entries.get(&title) else {
            continue;
        };

        for line in &entry.logs {
            let payload = format!("{title}\n{line}");
            (callback.callback)(task_id, &payload, "log", None, &0);
        }

        for diagnostic in &entry.diagnostics {
            let payload = format!("{title}\n{diagnostic}");
            (callback.callback)(task_id, &payload, "diagnostic", None, &0);
        }
    }
}
