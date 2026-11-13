//! Running an agent: start it, feed it the prompt, read its output as
//! events, and stop it when asked or when it takes too long.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::adapter::{Adapter, Task, Transcript};
use crate::event::{AgentEvent, Stream, Usage};

/// One run of an agent.
pub struct Run<'a> {
    pub adapter: &'a dyn Adapter,
    pub executable: PathBuf,
    pub task: Task<'a>,
    /// Set to stop the agent.
    pub cancel: Option<Arc<AtomicBool>>,
    /// Stop the agent after this long.
    pub timeout: Option<Duration>,
}

/// How a run that finished went.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub agent: &'static str,
    /// The agent's final answer.
    pub result: Option<String>,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub usage: Usage,
    pub duration: Duration,
}

/// Why a run did not finish. `agent` is the agent's label.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("cannot start {agent}: {source}")]
    Start {
        agent: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("{agent} failed: {message}")]
    Failed {
        agent: &'static str,
        message: String,
    },
    #[error("{agent} was stopped")]
    Canceled { agent: &'static str },
    #[error("{agent} did not finish within {}s", .after.as_secs())]
    TimedOut {
        agent: &'static str,
        after: Duration,
    },
}

const POLL: Duration = Duration::from_millis(100);
const STDERR_TAIL: usize = 20;

impl Run<'_> {
    /// Run the agent to the end. Every event goes to `on_event` as it
    /// happens.
    pub fn run(self, on_event: &mut dyn FnMut(AgentEvent)) -> Result<Outcome, RunError> {
        let agent = self.adapter.label();
        let started = Instant::now();
        let launch = self.adapter.launch(&self.task, &self.executable);

        let mut command = Command::new(&self.executable);
        command
            .args(&launch.args)
            .current_dir(self.task.cwd)
            .stdin(if launch.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("NO_COLOR", "1");
        // The agent starts a session of its own, not one inside ours.
        for key in self.adapter.session_env() {
            command.env_remove(key);
        }
        for (key, value) in &launch.env {
            command.env(key, value);
        }
        new_process_group(&mut command);

        let mut child = command
            .spawn()
            .map_err(|source| RunError::Start { agent, source })?;

        if let (Some(prompt), Some(mut stdin)) = (launch.stdin, child.stdin.take()) {
            // On its own thread: a full pipe must not block reading output.
            std::thread::spawn(move || {
                let _ = stdin.write_all(prompt.as_bytes());
                let _ = stdin.write_all(b"\n");
            });
        }
        let (sender, lines) = mpsc::channel();
        let readers = [
            child
                .stdout
                .take()
                .map(|out| read_lines(out, Stream::Stdout, sender.clone())),
            child
                .stderr
                .take()
                .map(|err| read_lines(err, Stream::Stderr, sender.clone())),
        ];
        drop(sender);

        let mut parser = self.adapter.parser();
        let mut transcript = Transcript::default();
        let mut stderr_tail: Vec<String> = Vec::new();
        loop {
            if self
                .cancel
                .as_ref()
                .is_some_and(|c| c.load(Ordering::SeqCst))
            {
                stop(&mut child);
                return Err(RunError::Canceled { agent });
            }
            if let Some(after) = self.timeout.filter(|t| started.elapsed() >= *t) {
                stop(&mut child);
                return Err(RunError::TimedOut { agent, after });
            }
            let (stream, line) = match lines.recv_timeout(POLL) {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            if line.trim().is_empty() {
                continue;
            }
            let events = match stream {
                Stream::Stdout => parser.parse(&line, &mut transcript),
                Stream::Stderr => {
                    if stderr_tail.len() == STDERR_TAIL {
                        stderr_tail.remove(0);
                    }
                    stderr_tail.push(line.clone());
                    vec![AgentEvent::Output { stream, line }]
                }
            };
            deliver(events, &mut transcript, on_event);
        }
        let events = parser.finish(&mut transcript);
        deliver(events, &mut transcript, on_event);
        for reader in readers.into_iter().flatten() {
            let _ = reader.join();
        }
        let status = child
            .wait()
            .map_err(|source| RunError::Start { agent, source })?;

        let finished = transcript.finish();
        if let Some(message) = finished.error {
            return Err(RunError::Failed { agent, message });
        }
        if !status.success() {
            let exit = match status.code() {
                Some(code) => format!("exited with code {code}"),
                None => format!("was stopped ({status})"),
            };
            let message = match stderr_tail.join("\n").trim() {
                "" => exit,
                tail => format!("{exit}: {tail}"),
            };
            return Err(RunError::Failed { agent, message });
        }
        Ok(Outcome {
            agent: self.adapter.id(),
            result: finished.result,
            session_id: finished.session_id,
            model: finished.model,
            usage: finished.usage,
            duration: started.elapsed(),
        })
    }
}

/// Hand events on, noting text for the final answer.
fn deliver(
    events: Vec<AgentEvent>,
    transcript: &mut Transcript,
    on_event: &mut dyn FnMut(AgentEvent),
) {
    for event in events {
        match &event {
            AgentEvent::Text { text, delta } => transcript.text(text, *delta),
            AgentEvent::ToolCall { .. } | AgentEvent::ToolResult { .. } => transcript.close_text(),
            _ => {}
        }
        on_event(event);
    }
}

/// Send each line of `reader` (lossily decoded, without ANSI escapes).
fn read_lines(
    reader: impl Read + Send + 'static,
    stream: Stream,
    sender: mpsc::Sender<(Stream, String)>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buffer);
                    let line = strip_ansi(line.trim_end_matches(['\n', '\r']));
                    if sender.send((stream, line)).is_err() {
                        break;
                    }
                }
            }
        }
    })
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            // CSI: ESC [ params final-byte
            Some('[') => {
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            // OSC: ESC ] ... (BEL | ESC \)
            Some(']') => {
                while let Some(next) = chars.next() {
                    if next == '\u{7}' || (next == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The agent and the commands it runs get their own process group, so
/// stopping the agent stops them too.
#[cfg(unix)]
fn new_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn new_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn stop(child: &mut Child) {
    let group = child.id() as libc::pid_t;
    // SAFETY: signalling a process group we created.
    unsafe { libc::killpg(group, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(POLL);
    }
    // SAFETY: as above.
    unsafe { libc::killpg(group, libc::SIGKILL) };
    let _ = child.wait();
}

#[cfg(not(unix))]
fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_colors_and_titles() {
        assert_eq!(strip_ansi("\u{1b}[1;32mok\u{1b}[0m done"), "ok done");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}text"), "text");
        assert_eq!(strip_ansi("plain"), "plain");
    }
}
