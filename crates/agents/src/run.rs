//! Running an agent: start it, feed it the prompt, read its output as
//! events, and stop it when asked or when it takes too long.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::adapter::{Adapter, Autonomy, ReadOnly, Task};
use crate::event::{AgentEvent, Stream, Usage};
use crate::transcript::Transcript;

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

/// What a run came to. For a run that did not finish (see
/// [`RunError::partial`]) this is what the agent had reported until then,
/// and `result` is the last text it wrote.
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
    /// Refused before anything was started.
    #[error("{agent} cannot be kept read-only, so it does not run read-only tasks")]
    ReadOnlyUnsupported { agent: &'static str },
    /// Refused before anything was started.
    #[error(
        "the prompt is too large for {agent}, which takes it on its command line: {} KiB, and at most {} KiB fit",
        .size / 1024,
        MAX_ARGUMENT / 1024
    )]
    PromptTooLarge { agent: &'static str, size: usize },
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
        partial: Box<Outcome>,
    },
    #[error("{agent} was stopped")]
    Canceled {
        agent: &'static str,
        partial: Box<Outcome>,
    },
    #[error("{agent} did not finish within {}s", .after.as_secs())]
    TimedOut {
        agent: &'static str,
        after: Duration,
        partial: Box<Outcome>,
    },
}

impl RunError {
    /// What the agent had reported before the run ended: its session, the
    /// usage so far and the last text it wrote. `None` when it never ran.
    pub fn partial(&self) -> Option<&Outcome> {
        match self {
            Self::Failed { partial, .. }
            | Self::Canceled { partial, .. }
            | Self::TimedOut { partial, .. } => Some(partial),
            Self::ReadOnlyUnsupported { .. } | Self::PromptTooLarge { .. } | Self::Start { .. } => {
                None
            }
        }
    }
}

/// Where the agent's process is while its output is read.
enum Phase {
    Running,
    /// It exited, at this time. Its pipes may still be open: something it
    /// started can hold them.
    Exited(ExitStatus, Instant),
}

/// How reading a run's output ended.
enum Ended {
    /// The agent exited, with its output read to the end or [`DRAIN`] over.
    Exited(ExitStatus),
    Canceled,
    TimedOut(Duration),
    /// The agent's process could not be waited for.
    Lost(std::io::Error),
}

const POLL: Duration = Duration::from_millis(100);
/// How long output is still read after the agent exited.
const DRAIN: Duration = Duration::from_secs(2);
/// How long a process group gets to end after SIGTERM, before SIGKILL.
const STOP: Duration = Duration::from_secs(3);
const STDERR_TAIL: usize = 20;
/// The largest command-line argument a run passes. Linux refuses one of
/// 128 KiB, and others limit the whole command line.
const MAX_ARGUMENT: usize = 100 * 1024;

impl Run<'_> {
    /// Run the agent to the end. Every event goes to `on_event` as it
    /// happens.
    pub fn run(self, on_event: &mut dyn FnMut(AgentEvent)) -> Result<Outcome, RunError> {
        let agent = self.adapter.label();
        if self.task.autonomy == Autonomy::ReadOnly
            && self.adapter.read_only() == ReadOnly::Unsupported
        {
            return Err(RunError::ReadOnlyUnsupported { agent });
        }
        let started = Instant::now();
        let launch = self.adapter.launch(&self.task, &self.executable);
        let largest = launch.args.iter().map(|arg| arg.len()).max().unwrap_or(0);
        if largest > MAX_ARGUMENT {
            return Err(RunError::PromptTooLarge {
                agent,
                size: largest,
            });
        }

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
        for key in self.adapter.env_markers() {
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
            // Not joined: it ends when the agent has read the prompt or is gone.
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

        let mut parser = launch.parser;
        let mut transcript = Transcript::default();
        let mut emit = |events: Vec<AgentEvent>, transcript: &mut Transcript| {
            for event in events {
                transcript.observe(&event);
                on_event(event);
            }
        };
        let mut stderr_tail: Vec<String> = Vec::new();
        let mut phase = Phase::Running;
        let mut pipes_open = true;
        let ended = loop {
            if self
                .cancel
                .as_ref()
                .is_some_and(|c| c.load(Ordering::SeqCst))
            {
                break Ended::Canceled;
            }
            if let Some(after) = self.timeout.filter(|t| started.elapsed() >= *t) {
                break Ended::TimedOut(after);
            }
            if let Phase::Running = phase {
                match child.try_wait() {
                    Ok(Some(status)) => phase = Phase::Exited(status, Instant::now()),
                    Ok(None) => {}
                    Err(error) => break Ended::Lost(error),
                }
            }
            if let Phase::Exited(status, at) = phase {
                if !pipes_open || at.elapsed() >= DRAIN {
                    break Ended::Exited(status);
                }
            }
            if !pipes_open {
                std::thread::sleep(POLL);
                continue;
            }
            let (stream, line) = match lines.recv_timeout(POLL) {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    pipes_open = false;
                    continue;
                }
            };
            let events = match stream {
                // What follows the agent's final report is not part of the run.
                Stream::Stdout if transcript.ended() => Vec::new(),
                Stream::Stdout => parser.parse(&line, &mut transcript),
                Stream::Stderr if line.trim().is_empty() => Vec::new(),
                Stream::Stderr => {
                    if stderr_tail.len() == STDERR_TAIL {
                        stderr_tail.remove(0);
                    }
                    stderr_tail.push(line.clone());
                    vec![AgentEvent::Output { stream, line }]
                }
            };
            emit(events, &mut transcript);
        };

        if pipes_open {
            // The agent is still running, or left something that holds its
            // pipes. The readers are not joined: a process that left the
            // group can keep a pipe open, and they end when it closes.
            stop_group(&mut child);
        } else {
            for reader in readers.into_iter().flatten() {
                let _ = reader.join();
            }
        }
        if let Ended::Exited(_) = ended {
            let events = parser.finish(&mut transcript);
            emit(events, &mut transcript);
        }

        let (outcome, reported) = transcript.into_outcome(self.adapter.id(), started.elapsed());
        let failed = |message: String, partial: Outcome| RunError::Failed {
            agent,
            message,
            partial: Box::new(partial),
        };
        match ended {
            Ended::Canceled => Err(RunError::Canceled {
                agent,
                partial: Box::new(outcome),
            }),
            Ended::TimedOut(after) => Err(RunError::TimedOut {
                agent,
                after,
                partial: Box::new(outcome),
            }),
            Ended::Lost(error) => Err(failed(
                format!("its process could not be waited for: {error}"),
                outcome,
            )),
            Ended::Exited(status) => match reported {
                Some(message) => Err(failed(message, outcome)),
                None if !status.success() => {
                    let exit = match status.code() {
                        Some(code) => format!("exited with code {code}"),
                        None => format!("was stopped ({status})"),
                    };
                    let message = match stderr_tail.join("\n").trim() {
                        "" => exit,
                        tail => format!("{exit}: {tail}"),
                    };
                    Err(failed(message, outcome))
                }
                None => Ok(outcome),
            },
        }
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

/// End the agent's process group: SIGTERM, then SIGKILL for whatever is
/// left of the group after [`STOP`], whether or not the agent itself is.
#[cfg(unix)]
fn stop_group(child: &mut Child) {
    let group = child.id() as libc::pid_t;
    // Whether the group still had a process to signal; signal 0 only asks.
    // SAFETY: signalling a process group we created.
    let signal = |signal: libc::c_int| unsafe { libc::killpg(group, signal) == 0 };
    signal(libc::SIGTERM);
    let deadline = Instant::now() + STOP;
    while Instant::now() < deadline {
        // The agent has to be reaped, or it stays in its group as a zombie.
        let reaped = !matches!(child.try_wait(), Ok(None));
        if reaped && !signal(0) {
            return;
        }
        std::thread::sleep(POLL);
    }
    signal(libc::SIGKILL);
    let _ = child.wait();
}

#[cfg(not(unix))]
fn stop_group(child: &mut Child) {
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
