use ratatui::style::Color;

/// Format duration from seconds
pub fn format_duration(seconds: i64) -> String {
    if seconds < 0 {
        return "-".to_string();
    }
    let secs = seconds as u64;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Truncate string to max length
pub fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}…", &s[..max_len.saturating_sub(1)])
    }
}

/// Strip ANSI escape codes from a string
pub fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // Start of ANSI escape sequence
            // Look for '[' which starts CSI sequence
            if let Some('[') = chars.next() {
                // Skip until we find a letter (the command)
                for next_ch in chars.by_ref() {
                    if next_ch.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Clean log line by removing timestamps and logging prefixes
pub fn clean_log_line(log: &str) -> String {
    let mut cleaned = log.trim();

    // Handle carriage returns - only keep text after the last \r
    if let Some(pos) = cleaned.rfind('\r') {
        cleaned = &cleaned[pos + 1..];
    }

    // Remove timestamp patterns like [2025-12-22T21:16:43Z]
    if let Some(pos) = cleaned.find(']') {
        if cleaned[..pos].contains('T') && cleaned[..pos].chars().any(|c| c.is_ascii_digit()) {
            cleaned = &cleaned[pos + 1..];
        }
    }

    // Remove logging level prefixes like [ERROR], [WARN], etc.
    for prefix in &["[ERROR]", "[WARN]", "[INFO]", "[DEBUG]", "[TRACE]"] {
        if cleaned.starts_with(prefix) {
            cleaned = &cleaned[prefix.len()..];
            break;
        }
    }

    // Remove "ERROR" word if it appears at the start
    if cleaned.starts_with("ERROR") {
        cleaned = &cleaned[5..];
    }

    // Remove module paths like butterflow_core::engine::
    if let Some(pos) = cleaned.find("::") {
        if let Some(pos2) = cleaned[pos + 2..].find("::") {
            if let Some(pos3) = cleaned[pos + 2 + pos2 + 2..].find(' ') {
                cleaned = &cleaned[pos + 2 + pos2 + 2 + pos3 + 1..];
            }
        }
    }

    // Remove "Task ... step ... failed:" prefix
    if let Some(pos) = cleaned.find("step ") {
        if let Some(pos2) = cleaned[pos..].find(" failed") {
            if let Some(pos3) = cleaned[pos + pos2 + 7..].find(':') {
                cleaned = &cleaned[pos + pos2 + 7 + pos3 + 1..];
            }
        }
    }

    // Remove "execution failed:" prefix
    if let Some(pos) = cleaned.find("execution failed:") {
        cleaned = &cleaned[pos + 17..];
    }
    // Trim whitespace
    cleaned.trim().to_string()
}

/// Get color for workflow status
pub fn status_color(status: butterflow_models::WorkflowStatus) -> Color {
    match status {
        butterflow_models::WorkflowStatus::Running => Color::Green,
        butterflow_models::WorkflowStatus::Completed => Color::Cyan,
        butterflow_models::WorkflowStatus::Failed => Color::Red,
        butterflow_models::WorkflowStatus::AwaitingTrigger => Color::Yellow,
        butterflow_models::WorkflowStatus::Canceled => Color::DarkGray,
        butterflow_models::WorkflowStatus::Pending => Color::Blue,
    }
}

/// Get color for task status
pub fn task_status_color(status: butterflow_models::TaskStatus) -> Color {
    match status {
        butterflow_models::TaskStatus::Running => Color::Green,
        butterflow_models::TaskStatus::Completed => Color::Cyan,
        butterflow_models::TaskStatus::Failed => Color::Red,
        butterflow_models::TaskStatus::AwaitingTrigger => Color::Yellow,
        butterflow_models::TaskStatus::Blocked => Color::Magenta,
        butterflow_models::TaskStatus::WontDo => Color::DarkGray,
        butterflow_models::TaskStatus::Pending => Color::Blue,
    }
}
