use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressState, ProgressStyle};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Truncate filename to a fixed width, keeping the end characters which are usually most relevant
fn truncate_filename(filename: &str, max_width: usize) -> String {
    if filename.len() <= max_width {
        // Pad with spaces to maintain consistent width
        format!("{filename:<max_width$}")
    } else {
        // Show "..." + last (max_width - 3) characters
        let suffix_len = max_width.saturating_sub(3);
        let suffix = &filename[filename.len().saturating_sub(suffix_len)..];
        let truncated = format!("...{suffix}");
        format!("{truncated:<max_width$}")
    }
}

pub fn download_progress_bar() -> Arc<Box<dyn Fn(u64, u64) + Send + Sync>> {
    let progress_bar = Arc::new(Mutex::new(None::<ProgressBar>));
    let progress_bar_clone = Arc::clone(&progress_bar);

    Arc::new(Box::new(move |downloaded: u64, total: u64| {
        let mut pb_guard = progress_bar_clone.lock().unwrap();
        if pb_guard.is_none() && total > 0 {
            let pb = ProgressBar::new(total);
            pb.set_style(
                ProgressStyle::with_template(
                    "{spinner:.green} [{elapsed_precise}] [{wide_bar:.white/blue}] {bytes}/{total_bytes} ({eta})"
                )
                .unwrap()
                .with_key("eta", |state: &ProgressState, w: &mut dyn std::fmt::Write| {
                    write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap()
                })
                .progress_chars("━╸ ")
            );
            *pb_guard = Some(pb);
        }
        if let Some(ref pb) = *pb_guard {
            pb.set_position(downloaded);
            if downloaded >= total {
                pb.finish_with_message("Downloaded successfully");
            }
        }
    }))
}

pub enum ProgressAction {
    Start { total_files: Option<u64> },
    Update { current_file: String },
    Log { title: String, line: String },
    Diagnostic { title: String, message: String },
    Increment,
    Finish { message: Option<String> },
}

pub struct ProgressUpdate {
    pub task_id: String,
    pub action: ProgressAction,
}

pub type ProgressReporter = Arc<Box<dyn Fn(ProgressUpdate) + Send + Sync + 'static>>;

pub fn create_multi_progress_reporter() -> (ProgressReporter, Instant) {
    let started = Instant::now();

    // Define styles for different progress bar states
    let progress_style = ProgressStyle::with_template(
        "{elapsed_precise:.dim} {bar:40.cyan/blue} {pos:>7}/{len:<7} {msg}",
    )
    .unwrap()
    .progress_chars("━╸ ");

    let spinner_style = ProgressStyle::with_template(
        "{prefix:.bold.cyan} {spinner:.cyan} {msg} {elapsed_precise:.dim}",
    )
    .unwrap()
    .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");

    let multi_progress = Arc::new(MultiProgress::new());
    let progress_bars = Arc::new(Mutex::new(HashMap::<String, ProgressBar>::new()));
    let active_log_title = Arc::new(Mutex::new(None::<String>));

    // Enable stderr output
    multi_progress.set_draw_target(indicatif::ProgressDrawTarget::stderr());

    let reporter: ProgressReporter = Arc::new(Box::new(move |update: ProgressUpdate| {
        let bars = Arc::clone(&progress_bars);
        let mp = Arc::clone(&multi_progress);
        let active_log_title = Arc::clone(&active_log_title);
        let task_id = update.task_id.clone();

        match update.action {
            ProgressAction::Start { total_files } => {
                let mut bars_lock = bars.lock().unwrap();
                *active_log_title.lock().unwrap() = None;

                // Remove existing bar if it exists
                if let Some(existing_pb) = bars_lock.remove(&task_id) {
                    mp.remove(&existing_pb);
                }

                let pb = if let Some(total) = total_files {
                    let pb = mp.add(ProgressBar::new(total));
                    pb.set_style(progress_style.clone());
                    pb.set_prefix(task_id.clone());
                    pb.set_message(style("Starting").dim().to_string());
                    pb
                } else {
                    let pb = mp.add(ProgressBar::new_spinner());
                    pb.set_style(spinner_style.clone());
                    pb.set_prefix(task_id.clone());
                    pb.set_message(style("Starting").dim().to_string());
                    pb
                };

                bars_lock.insert(task_id, pb);
            }

            ProgressAction::Update { current_file } => {
                let bars_lock = bars.lock().unwrap();
                if let Some(pb) = bars_lock.get(&task_id) {
                    let filename = std::path::Path::new(&current_file)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    let truncated_filename = truncate_filename(&filename, 25);
                    pb.set_message(style(truncated_filename).cyan().to_string());
                    pb.tick();
                }
            }

            ProgressAction::Increment => {
                let bars_lock = bars.lock().unwrap();
                if let Some(pb) = bars_lock.get(&task_id) {
                    pb.inc(1);
                }
            }

            ProgressAction::Log { title, line } => {
                if line.trim().is_empty() {
                    return;
                }

                let mut active_title = active_log_title.lock().unwrap();
                mp.suspend(|| {
                    if active_title.as_deref() != Some(title.as_str()) {
                        eprintln!();
                        eprintln!("{}", style(&title).bold().cyan());
                        *active_title = Some(title.clone());
                    }
                    for line in line.lines() {
                        eprintln!("  {line}");
                    }
                });
            }

            ProgressAction::Diagnostic { title, message } => {
                if message.trim().is_empty() {
                    return;
                }

                let rendered = crate::diagnostics::render_error_message(&message);
                let mut active_title = active_log_title.lock().unwrap();
                mp.suspend(|| {
                    if active_title.as_deref() != Some(title.as_str()) {
                        eprintln!();
                        eprintln!("{}", style(&title).bold().cyan());
                        *active_title = Some(title.clone());
                    }
                    for line in rendered.lines() {
                        eprintln!("  {line}");
                    }
                });
            }

            ProgressAction::Finish { message } => {
                let bars_lock = bars.lock().unwrap();
                *active_log_title.lock().unwrap() = None;
                if let Some(pb) = bars_lock.get(&task_id) {
                    let finish_message = message.unwrap_or_else(|| "Completed".to_string());
                    pb.finish_with_message(style(finish_message).green().to_string());
                }
            }
        }
    }));

    (reporter, started)
}
