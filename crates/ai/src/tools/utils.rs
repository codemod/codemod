//! Utility functions shared across codemod-ai tools.

use crate::tools::core::Result;
use std::path::Path;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

/// Maximum response length before truncation.
pub const MAX_RESPONSE_LEN: usize = 16000;

/// Message appended when output is truncated.
pub const TRUNCATED_MESSAGE: &str = "<response clipped><NOTE>To save on context only part of this file has been shown to you. You should retry this tool after you have searched inside the file with `grep -n` in order to find the line numbers of what you are looking for.</NOTE>";

/// Truncate content if it exceeds the provided length.
pub fn maybe_truncate(content: &str, truncate_after: Option<usize>) -> String {
    let limit = truncate_after.unwrap_or(MAX_RESPONSE_LEN);
    if content.chars().count() <= limit {
        content.to_string()
    } else {
        let clipped: String = content.chars().take(limit).collect();
        format!("{clipped}{TRUNCATED_MESSAGE}")
    }
}

/// Run a shell command asynchronously with timeout.
pub async fn run_command(
    cmd: &str,
    timeout_secs: Option<u64>,
    truncate_after: Option<usize>,
) -> Result<(i32, String, String)> {
    let timeout_duration = Duration::from_secs(timeout_secs.unwrap_or(120));

    let result = timeout(timeout_duration, async {
        let output = Command::new("sh").arg("-c").arg(cmd).output().await?;

        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        Ok::<(i32, String, String), std::io::Error>((exit_code, stdout, stderr))
    })
    .await;

    match result {
        Ok(Ok((exit_code, stdout, stderr))) => Ok((
            exit_code,
            maybe_truncate(&stdout, truncate_after),
            maybe_truncate(&stderr, truncate_after),
        )),
        Ok(Err(e)) => Err(e.into()),
        Err(_) => Err(format!(
            "Command '{}' timed out after {} seconds",
            cmd,
            timeout_secs.unwrap_or(120)
        )
        .into()),
    }
}

/// Format content with line numbers.
pub fn format_with_line_numbers(content: &str, start_line: usize) -> String {
    content
        .lines()
        .enumerate()
        .map(|(i, line)| format!("{:6}\t{}", i + start_line, line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Validate that a path is absolute.
pub fn validate_absolute_path(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        let suggested_path = Path::new("/").join(path);
        return Err(format!(
            "The path {} is not an absolute path, it should start with `/`. Maybe you meant {}?",
            path.display(),
            suggested_path.display()
        )
        .into());
    }
    Ok(())
}

/// Check if a file exists and return an operation-specific error.
pub fn check_file_exists(path: &Path, operation: &str) -> Result<()> {
    match operation {
        "create" => {
            if path.exists() {
                return Err(format!(
                    "File already exists at: {}. Cannot overwrite files using command `create`.",
                    path.display()
                )
                .into());
            }
        }
        _ => {
            if !path.exists() {
                return Err(format!(
                    "The path {} does not exist. Please provide a valid path.",
                    path.display()
                )
                .into());
            }
        }
    }
    Ok(())
}

/// Validate operations against directory paths.
pub fn validate_directory_operation(path: &Path, operation: &str) -> Result<()> {
    if path.is_dir() && operation != "view" {
        return Err(format!(
            "The path {} is a directory and only the `view` command can be used on directories",
            path.display()
        )
        .into());
    }
    Ok(())
}

/// Expand tabs into four spaces.
pub fn expand_tabs(content: &str) -> String {
    content.replace('\t', "    ")
}

/// Build a snippet around a target line.
pub fn create_edit_snippet(content: &str, target_line: usize, snippet_lines: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let start_line = target_line.saturating_sub(snippet_lines);
    let end_line = std::cmp::min(target_line + snippet_lines + 1, lines.len());

    lines[start_line..end_line]
        .iter()
        .enumerate()
        .map(|(i, line)| format!("{:6}\t{}", start_line + i + 1, line))
        .collect::<Vec<_>>()
        .join("\n")
}
