use std::collections::{HashMap, HashSet};

const MAX_PARENT_DEPTH: usize = 8;

const KNOWN_AGENTS: &[KnownAgent] = &[
    KnownAgent {
        canonical: "codex",
        aliases: &["codex", "codex-cli"],
    },
    KnownAgent {
        canonical: "claude-code",
        aliases: &["claude", "claude-code"],
    },
    KnownAgent {
        canonical: "aider",
        aliases: &["aider"],
    },
    KnownAgent {
        canonical: "cursor",
        aliases: &["cursor", "cursor-agent"],
    },
    KnownAgent {
        canonical: "windsurf",
        aliases: &["windsurf"],
    },
    KnownAgent {
        canonical: "goose",
        aliases: &["goose", "goose-cli", "block-goose"],
    },
    KnownAgent {
        canonical: "opencode",
        aliases: &["opencode", "opencode-cli", "open-code"],
    },
    KnownAgent {
        canonical: "openclaw",
        aliases: &["openclaw", "openclaw-cli", "open-claw"],
    },
];

const ENV_MARKERS: &[EnvMarker] = &[
    EnvMarker {
        key: "CODEX_HOME",
        agent: "codex",
    },
    EnvMarker {
        key: "CODEX_SESSION_ID",
        agent: "codex",
    },
    EnvMarker {
        key: "CODEX_SANDBOX",
        agent: "codex",
    },
    EnvMarker {
        key: "CLAUDE_CODE",
        agent: "claude-code",
    },
    EnvMarker {
        key: "CLAUDE_CODE_SESSION_ID",
        agent: "claude-code",
    },
    EnvMarker {
        key: "AIDER_MODEL",
        agent: "aider",
    },
    EnvMarker {
        key: "AIDER_CHAT_HISTORY_FILE",
        agent: "aider",
    },
    EnvMarker {
        key: "CURSOR_AGENT",
        agent: "cursor",
    },
    EnvMarker {
        key: "WINDSURF_SESSION_ID",
        agent: "windsurf",
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionConfidence {
    Detected,
    Uncertain,
    NotDetected,
}

impl DetectionConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Detected => "detected",
            Self::Uncertain => "uncertain",
            Self::NotDetected => "not_detected",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionResult {
    pub confidence: DetectionConfidence,
    pub agent_name: Option<String>,
    pub reasons: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SignalStrength {
    Strong,
    Medium,
    Weak,
}

#[derive(Clone, Debug)]
struct Signal {
    agent: &'static str,
    strength: SignalStrength,
    reason: String,
}

#[derive(Clone, Copy)]
struct KnownAgent {
    canonical: &'static str,
    aliases: &'static [&'static str],
}

#[derive(Clone, Copy)]
struct EnvMarker {
    key: &'static str,
    agent: &'static str,
}

#[derive(Debug)]
enum ProcessInspection {
    Signals(Vec<Signal>),
    Failed(String),
}

#[derive(Debug)]
struct ProcessSnapshot {
    pid: u32,
    parent_pid: u32,
    executable: String,
    command: String,
}

pub fn detect_parent_coding_agent() -> DetectionResult {
    let env_signals = collect_env_signals(std::env::vars());
    let process_signals = collect_process_signals(MAX_PARENT_DEPTH);
    classify_detection(process_signals, env_signals)
}

fn classify_detection(
    process_signals: ProcessInspection,
    env_signals: Vec<Signal>,
) -> DetectionResult {
    let mut signals = env_signals;
    let mut reasons = Vec::new();
    let mut process_failed = false;

    match process_signals {
        ProcessInspection::Signals(mut process) => {
            signals.append(&mut process);
        }
        ProcessInspection::Failed(error) => {
            process_failed = true;
            reasons.push(format!("process inspection failed: {error}"));
        }
    }

    dedupe_signals(&mut signals);
    reasons.extend(signals.iter().map(|s| s.reason.clone()));
    let best_agent = choose_best_agent(&signals).map(str::to_string);

    if process_failed {
        return DetectionResult {
            confidence: DetectionConfidence::Uncertain,
            agent_name: best_agent,
            reasons,
        };
    }

    if signals
        .iter()
        .any(|signal| signal.strength == SignalStrength::Strong)
    {
        return DetectionResult {
            confidence: DetectionConfidence::Detected,
            agent_name: best_agent,
            reasons,
        };
    }

    let non_strong = signals
        .iter()
        .filter(|signal| signal.strength != SignalStrength::Strong)
        .count();

    let confidence = if non_strong >= 2 {
        DetectionConfidence::Detected
    } else if non_strong == 1 {
        DetectionConfidence::Uncertain
    } else {
        DetectionConfidence::NotDetected
    };

    DetectionResult {
        confidence,
        agent_name: best_agent,
        reasons,
    }
}

fn dedupe_signals(signals: &mut Vec<Signal>) {
    let mut seen = HashSet::new();
    signals.retain(|signal| seen.insert(signal.reason.clone()));
}

fn choose_best_agent(signals: &[Signal]) -> Option<&'static str> {
    if signals.is_empty() {
        return None;
    }

    let mut counts = HashMap::new();
    for signal in signals {
        *counts.entry(signal.agent).or_insert(0usize) += 1;
    }

    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(agent, _)| agent)
}

fn collect_env_signals<I>(vars: I) -> Vec<Signal>
where
    I: IntoIterator<Item = (String, String)>,
{
    let env_map: HashMap<String, String> = vars.into_iter().collect();
    let mut signals = Vec::new();

    for marker in ENV_MARKERS {
        if let Some(value) = env_map.get(marker.key) {
            if !value.trim().is_empty() {
                signals.push(Signal {
                    agent: marker.agent,
                    strength: SignalStrength::Weak,
                    reason: format!("env marker {} indicates {}", marker.key, marker.agent),
                });
            }
        }
    }

    signals
}

fn collect_process_signals(max_parent_depth: usize) -> ProcessInspection {
    match inspect_parent_process_chain(max_parent_depth) {
        Ok(chain) => {
            let mut signals = Vec::new();

            for process in chain {
                let executable_path = process.executable.trim();
                if !executable_path.is_empty() {
                    if let Some(agent) = detect_agent_in_executable(executable_path) {
                        signals.push(Signal {
                            agent,
                            strength: SignalStrength::Strong,
                            reason: format!(
                                "parent process pid={} ppid={} executable={} matched {}",
                                process.pid, process.parent_pid, executable_path, agent
                            ),
                        });
                    }
                }

                if let Some(agent) = detect_agent_in_text(&process.command) {
                    signals.push(Signal {
                        agent,
                        strength: SignalStrength::Medium,
                        reason: format!(
                            "parent process pid={} command matched {}",
                            process.pid, agent
                        ),
                    });
                }
            }

            ProcessInspection::Signals(signals)
        }
        Err(error) => ProcessInspection::Failed(error),
    }
}

fn detect_agent_in_executable(executable: &str) -> Option<&'static str> {
    let slash_tail = executable
        .rsplit_once('/')
        .map_or(executable, |(_, tail)| tail);
    let basename = slash_tail
        .rsplit_once('\\')
        .map_or(slash_tail, |(_, tail)| tail);

    detect_agent_in_text(basename)
}

fn detect_agent_in_text(text: &str) -> Option<&'static str> {
    let normalized_text = normalize_text_for_matching(text);

    for known_agent in KNOWN_AGENTS {
        if known_agent
            .aliases
            .iter()
            .any(|alias| contains_alias(&normalized_text, alias))
        {
            return Some(known_agent.canonical);
        }
    }

    None
}

fn contains_alias(normalized_text: &str, alias: &str) -> bool {
    let normalized_alias = normalize_text_for_matching(alias);
    if normalized_alias.is_empty() || normalized_text.is_empty() {
        return false;
    }

    if normalized_text == normalized_alias {
        return true;
    }

    normalized_text.starts_with(&(normalized_alias.clone() + " "))
        || normalized_text.contains(&format!(" {normalized_alias} "))
        || normalized_text.ends_with(&format!(" {normalized_alias}"))
}

fn normalize_text_for_matching(input: &str) -> String {
    let lower = input.to_ascii_lowercase().replace(".exe", " ");
    let mut normalized = String::with_capacity(lower.len());
    let mut last_was_space = false;

    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch);
            last_was_space = false;
            continue;
        }

        if !last_was_space {
            normalized.push(' ');
            last_was_space = true;
        }
    }

    normalized.trim().to_string()
}

#[cfg(unix)]
fn inspect_parent_process_chain(
    max_parent_depth: usize,
) -> std::result::Result<Vec<ProcessSnapshot>, String> {
    use std::process::Command;

    let mut chain = Vec::new();
    let mut seen_pids = HashSet::new();
    let mut pid = unsafe { libc::getppid() as i32 };

    for _ in 0..max_parent_depth {
        if pid <= 1 || !seen_pids.insert(pid) {
            break;
        }

        let output = Command::new("ps")
            .args([
                "-ww",
                "-p",
                &pid.to_string(),
                "-o",
                "ppid=",
                "-o",
                "comm=",
                "-o",
                "args=",
            ])
            .output()
            .map_err(|error| format!("failed to inspect parent process {pid}: {error}"))?;

        if !output.status.success() {
            return Err(format!(
                "ps command failed while inspecting parent process {pid}"
            ));
        }

        let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if line.is_empty() {
            break;
        }

        let mut parts = line.split_whitespace();
        let parent_pid = parts
            .next()
            .ok_or_else(|| format!("unable to parse ppid from process line: {line}"))?
            .parse::<i32>()
            .map_err(|error| format!("invalid parent pid in process line '{line}': {error}"))?;
        let executable = parts
            .next()
            .ok_or_else(|| format!("unable to parse executable from process line: {line}"))?
            .to_string();
        let command = parts.collect::<Vec<_>>().join(" ");

        chain.push(ProcessSnapshot {
            pid: pid as u32,
            parent_pid: parent_pid.max(0) as u32,
            executable: executable.clone(),
            command: if command.is_empty() {
                executable
            } else {
                command
            },
        });

        pid = parent_pid;
    }

    Ok(chain)
}

#[cfg(not(unix))]
fn inspect_parent_process_chain(
    _max_parent_depth: usize,
) -> std::result::Result<Vec<ProcessSnapshot>, String> {
    Err("parent-process inspection is unavailable on this platform".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weak(agent: &'static str, reason: &str) -> Signal {
        Signal {
            agent,
            strength: SignalStrength::Weak,
            reason: reason.to_string(),
        }
    }

    fn medium(agent: &'static str, reason: &str) -> Signal {
        Signal {
            agent,
            strength: SignalStrength::Medium,
            reason: reason.to_string(),
        }
    }

    fn strong(agent: &'static str, reason: &str) -> Signal {
        Signal {
            agent,
            strength: SignalStrength::Strong,
            reason: reason.to_string(),
        }
    }

    #[test]
    fn strong_parent_match_is_detected() {
        let result = classify_detection(
            ProcessInspection::Signals(vec![strong("codex", "process match codex")]),
            vec![],
        );
        assert_eq!(result.confidence, DetectionConfidence::Detected);
        assert_eq!(result.agent_name.as_deref(), Some("codex"));
    }

    #[test]
    fn weak_only_signal_is_uncertain() {
        let result = classify_detection(
            ProcessInspection::Signals(vec![]),
            vec![weak("codex", "env marker")],
        );
        assert_eq!(result.confidence, DetectionConfidence::Uncertain);
        assert_eq!(result.agent_name.as_deref(), Some("codex"));
    }

    #[test]
    fn no_signals_is_not_detected() {
        let result = classify_detection(ProcessInspection::Signals(vec![]), vec![]);
        assert_eq!(result.confidence, DetectionConfidence::NotDetected);
        assert!(result.agent_name.is_none());
    }

    #[test]
    fn boundary_matching_rejects_substrings() {
        assert_eq!(detect_agent_in_text("codec-service"), None);
        assert_eq!(detect_agent_in_text("mycodexhelper"), None);
        assert_eq!(detect_agent_in_text("codex-cli"), Some("codex"));
    }

    #[test]
    fn detects_goose_opencode_and_openclaw() {
        assert_eq!(detect_agent_in_text("goose"), Some("goose"));
        assert_eq!(detect_agent_in_text("goose-cli run"), Some("goose"));
        assert_eq!(detect_agent_in_text("block-goose"), Some("goose"));
        assert_eq!(detect_agent_in_text("opencode --agent"), Some("opencode"));
        assert_eq!(detect_agent_in_text("open-code"), Some("opencode"));
        assert_eq!(detect_agent_in_text("opencode-cli"), Some("opencode"));
        assert_eq!(
            detect_agent_in_text("/usr/local/bin/openclaw"),
            Some("openclaw")
        );
        assert_eq!(detect_agent_in_text("open-claw"), Some("openclaw"));
        assert_eq!(detect_agent_in_text("openclaw-cli"), Some("openclaw"));
    }

    #[test]
    fn process_failure_downgrades_to_uncertain() {
        let result = classify_detection(
            ProcessInspection::Failed("access denied".to_string()),
            vec![weak("codex", "env marker")],
        );
        assert_eq!(result.confidence, DetectionConfidence::Uncertain);
        assert_eq!(result.agent_name.as_deref(), Some("codex"));
    }

    #[test]
    fn two_non_strong_signals_are_detected() {
        let result = classify_detection(
            ProcessInspection::Signals(vec![medium("codex", "cmdline"), weak("codex", "env")]),
            vec![],
        );
        assert_eq!(result.confidence, DetectionConfidence::Detected);
        assert_eq!(result.agent_name.as_deref(), Some("codex"));
    }
}
