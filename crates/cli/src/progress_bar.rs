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
    Start {
        total_files: Option<u64>,
        label: Option<String>,
    },
    Update {
        current_file: String,
    },
    Agent {
        payload: String,
    },
    Log {
        title: String,
        line: String,
    },
    Diagnostic {
        title: String,
        message: String,
    },
    Increment,
    Finish {
        message: Option<String>,
    },
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

    let spinner_style = ProgressStyle::with_template("{spinner:.cyan} {msg}")
        .unwrap()
        .tick_chars("⠈⠉⠋⠓⠒⠐⠐⠒⠖⠦⠤⠠⠠⠤⠦⠖⠒⠐⠐⠒⠓⠋⠉⠈ ");

    let multi_progress = Arc::new(MultiProgress::new());
    let progress_bars = Arc::new(Mutex::new(HashMap::<String, ProgressBar>::new()));
    let agent_spinners = Arc::new(Mutex::new(HashMap::<String, ProgressBar>::new()));
    let agent_message_buffers = Arc::new(Mutex::new(HashMap::<String, String>::new()));
    let agent_message_open = Arc::new(Mutex::new(HashMap::<String, bool>::new()));
    let active_log_title = Arc::new(Mutex::new(None::<String>));

    // Enable stderr output
    multi_progress.set_draw_target(indicatif::ProgressDrawTarget::stderr());

    let reporter: ProgressReporter = Arc::new(Box::new(move |update: ProgressUpdate| {
        let bars = Arc::clone(&progress_bars);
        let agent_spinners = Arc::clone(&agent_spinners);
        let agent_message_buffers = Arc::clone(&agent_message_buffers);
        let agent_message_open = Arc::clone(&agent_message_open);
        let mp = Arc::clone(&multi_progress);
        let active_log_title = Arc::clone(&active_log_title);
        let task_id = update.task_id.clone();

        match update.action {
            ProgressAction::Start { total_files, label } => {
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
                    let message = label
                        .map(|label| format!("Running {label}"))
                        .unwrap_or_else(|| "Starting".to_string());
                    pb.set_message(style(message).dim().to_string());
                    pb
                } else {
                    let pb = mp.add(ProgressBar::new_spinner());
                    pb.set_style(spinner_style.clone());
                    pb.set_prefix(task_id.clone());
                    let message = label
                        .map(|label| format!("Running {label}"))
                        .unwrap_or_else(|| "Starting".to_string());
                    pb.set_message(style(message).dim().to_string());
                    pb.enable_steady_tick(std::time::Duration::from_millis(120));
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

            ProgressAction::Agent { payload } => {
                if payload.trim().is_empty() {
                    return;
                }
                let value = serde_json::from_str::<serde_json::Value>(&payload).ok();
                if is_agent_starting_event(value.as_ref()) {
                    let agent = value
                        .as_ref()
                        .and_then(|value| value.get("agent"))
                        .and_then(serde_json::Value::as_str)
                        .map(agent_display_name)
                        .unwrap_or("Agent");
                    if let Some(pb) = bars.lock().unwrap().get(&task_id) {
                        pb.set_message(style(format!("{agent} is thinking...")).dim().to_string());
                        pb.tick();
                    } else {
                        start_agent_spinner(&mp, &agent_spinners, &spinner_style, &task_id, agent);
                    }
                    return;
                }
                if let Some(pb) = bars.lock().unwrap().get(&task_id) {
                    pb.tick();
                }

                if let Some(delta) = agent_message_delta(value.as_ref()) {
                    clear_agent_spinner(&agent_spinners, &task_id);
                    let was_open = *agent_message_open
                        .lock()
                        .unwrap()
                        .get(&task_id)
                        .unwrap_or(&false);
                    let fragment =
                        crate::agent_log_renderer::render_agent_message_fragment(delta, was_open);
                    if !fragment.is_empty() {
                        mp.suspend(|| {
                            eprint!("{fragment}");
                        });
                        let is_open = !fragment.ends_with('\n');
                        agent_message_open
                            .lock()
                            .unwrap()
                            .insert(task_id.clone(), is_open);
                        if !is_open {
                            let agent = value
                                .as_ref()
                                .and_then(|value| value.get("agent"))
                                .and_then(serde_json::Value::as_str)
                                .map(agent_display_name)
                                .unwrap_or("Agent");
                            start_agent_spinner(
                                &mp,
                                &agent_spinners,
                                &spinner_style,
                                &task_id,
                                agent,
                            );
                        }
                    }
                    return;
                }

                let Some(rendered) =
                    crate::agent_log_renderer::render_agent_event_payload_styled(&payload, false)
                else {
                    return;
                };
                match rendered {
                    crate::agent_log_renderer::RenderedAgentEvent::Line(line) => {
                        clear_agent_spinner(&agent_spinners, &task_id);
                        mp.suspend(|| {
                            if agent_message_open
                                .lock()
                                .unwrap()
                                .insert(task_id.clone(), false)
                                .unwrap_or(false)
                            {
                                eprintln!();
                            }
                            if let Some(message) =
                                agent_message_buffers.lock().unwrap().remove(&task_id)
                            {
                                if !message.trim().is_empty() {
                                    eprintln!("  {} {}", style("›").cyan(), message.trim_end());
                                }
                            }
                            eprintln!("{line}");
                        });
                        if !bars.lock().unwrap().contains_key(&task_id) {
                            let agent = value
                                .as_ref()
                                .and_then(|value| value.get("agent"))
                                .and_then(serde_json::Value::as_str)
                                .map(agent_display_name)
                                .unwrap_or("Agent");
                            start_agent_spinner(
                                &mp,
                                &agent_spinners,
                                &spinner_style,
                                &task_id,
                                agent,
                            );
                        }
                    }
                    crate::agent_log_renderer::RenderedAgentEvent::Fragment(fragment) => {
                        if fragment.is_empty() {
                            return;
                        }
                        agent_message_buffers
                            .lock()
                            .unwrap()
                            .entry(task_id.clone())
                            .or_default()
                            .push_str(&fragment);
                    }
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
                let mut bars_lock = bars.lock().unwrap();
                *active_log_title.lock().unwrap() = None;
                if let Some(message) = agent_message_buffers.lock().unwrap().remove(&task_id) {
                    if !message.trim().is_empty() {
                        clear_agent_spinner(&agent_spinners, &task_id);
                        mp.suspend(|| {
                            eprintln!("  {} {}", style("›").cyan(), message.trim_end());
                        });
                    }
                }
                if agent_message_open
                    .lock()
                    .unwrap()
                    .remove(&task_id)
                    .unwrap_or(false)
                {
                    mp.suspend(|| eprintln!());
                }
                clear_agent_spinner(&agent_spinners, &task_id);
                if let Some(pb) = bars_lock.remove(&task_id) {
                    let finish_message = message.unwrap_or_else(|| "Completed".to_string());
                    pb.finish_with_message(style(finish_message).green().to_string());
                }
            }
        }
    }));

    (reporter, started)
}

fn is_agent_starting_event(value: Option<&serde_json::Value>) -> bool {
    value
        .and_then(|value| value.get("event"))
        .and_then(serde_json::Value::as_str)
        == Some("status")
        && value
            .and_then(|value| value.get("status"))
            .and_then(serde_json::Value::as_str)
            == Some("starting")
}

fn agent_message_delta(value: Option<&serde_json::Value>) -> Option<&str> {
    let value = value?;
    if value.get("event").and_then(serde_json::Value::as_str) != Some("message") {
        return None;
    }
    if !value
        .get("delta")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    value.get("text").and_then(serde_json::Value::as_str)
}

fn start_agent_spinner(
    multi_progress: &MultiProgress,
    agent_spinners: &Arc<Mutex<HashMap<String, ProgressBar>>>,
    spinner_style: &ProgressStyle,
    task_id: &str,
    agent: &str,
) {
    let mut spinners = agent_spinners.lock().unwrap();
    spinners.entry(task_id.to_string()).or_insert_with(|| {
        let pb = multi_progress.add(ProgressBar::new_spinner());
        pb.set_style(spinner_style.clone());
        pb.set_prefix(style("AI").bold().cyan().to_string());
        pb.set_message(style(format!("{agent} is thinking...")).dim().to_string());
        pb.enable_steady_tick(std::time::Duration::from_millis(120));
        pb
    });
}

fn clear_agent_spinner(agent_spinners: &Arc<Mutex<HashMap<String, ProgressBar>>>, task_id: &str) {
    if let Some(spinner) = agent_spinners.lock().unwrap().remove(task_id) {
        spinner.finish_and_clear();
    }
}

fn agent_display_name(canonical: &str) -> &str {
    match canonical {
        "claude-code" => "Claude Code",
        "opencode" => "OpenCode",
        "codex" => "Codex",
        _ => canonical,
    }
}
