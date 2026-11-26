//! Drawing the setup and run screens.

use codemod_workflow::interaction::Approval;
use codemod_workflow::RunStatus;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::setup::{Field, SetupState};
use super::view::{Modal, Pane, Phase, RunView, StepState};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

fn footer(frame: &mut Frame, area: Rect, keys: &[(&str, &str)]) {
    let mut spans = Vec::new();
    for (i, (key, what)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", dim()));
        }
        spans.push(Span::styled(*key, bold()));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(*what, dim()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

// --------------------------------------------------------------- setup ---

pub fn setup(frame: &mut Frame, state: &SetupState) {
    let [body, foot] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(frame.area());
    let focused = state.focused();

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("Workflow  ", dim()),
            Span::raw(state.workflow.display().to_string()),
        ]),
        Line::raw(""),
    ];
    let label_width = 16;
    let row = |field: Field, label: String, value: Vec<Span<'static>>, hint: Option<String>| {
        let is_focused = field == focused;
        let marker = if is_focused { "› " } else { "  " };
        let mut spans = vec![
            Span::styled(marker, Style::default().fg(Color::Cyan)),
            Span::styled(
                format!("{label:<label_width$}"),
                if is_focused { bold() } else { Style::default() },
            ),
        ];
        if is_focused {
            if let Some(text) = &state.editing {
                spans.push(Span::styled(
                    format!("{text}▏"),
                    Style::default().fg(Color::Yellow),
                ));
                return Line::from(spans);
            }
        }
        spans.extend(value);
        if let Some(hint) = hint {
            spans.push(Span::styled(format!("  {hint}"), dim()));
        }
        Line::from(spans)
    };
    let check = |on: bool| {
        if on {
            Span::styled("[x]", Style::default().fg(Color::Green))
        } else {
            Span::raw("[ ]")
        }
    };
    let choice = |text: String| {
        vec![
            Span::styled("‹ ", dim()),
            Span::styled(text, Style::default().fg(Color::Cyan)),
            Span::styled(" ›", dim()),
        ]
    };

    lines.push(row(
        Field::Target,
        "Target".into(),
        vec![Span::raw(state.target.clone())],
        None,
    ));
    lines.push(row(
        Field::DryRun,
        "Dry run".into(),
        vec![check(state.dry_run)],
        Some(if state.dry_run_locked {
            "required for this codemod".into()
        } else if state.dry_run {
            "preview the changes; apply them afterwards".into()
        } else {
            "changes are written".into()
        }),
    ));

    if !state.params.is_empty() {
        lines.push(Line::styled("  Params", dim()));
        for (i, param) in state.params.iter().enumerate() {
            let value = if param.value.is_empty() {
                Span::styled("—", dim())
            } else {
                Span::raw(param.value.clone())
            };
            let hint = [param.type_text.clone(), param.description.clone()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            lines.push(row(
                Field::Param(i),
                format!("  {}", param.name),
                vec![value],
                Some(hint).filter(|h| !h.is_empty()),
            ));
        }
    }

    let agent = &state.agents[state.agent];
    lines.push(row(
        Field::Agent,
        "Coding agent".into(),
        choice(agent.label.clone()),
        Some(if !agent.installed {
            "not installed".into()
        } else if state.agents.len() == 1 {
            "no coding agent found on PATH".into()
        } else {
            format!("{} installed", state.agents.len() - 1)
        }),
    ));
    lines.push(row(
        Field::Model,
        "Decision model".into(),
        choice(format!("{}/{}", state.provider, state.model_name())),
        Some(if state.fetching_models {
            "fetching models…".into()
        } else {
            format!(
                "{} model{}",
                state.models.len(),
                if state.models.len() == 1 { "" } else { "s" }
            )
        }),
    ));
    lines.push(Line::raw(""));
    let start_style = if focused == Field::Start {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Green)
    };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            if state.dry_run {
                " Start dry run "
            } else {
                " Start run "
            },
            start_style,
        ),
    ]));
    if let Some(error) = &state.error {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            format!("  {error}"),
            Style::default().fg(Color::Red),
        ));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(vec![
            Span::styled(" codemod ", bold()),
            Span::styled(format!("· {} ", state.title), dim()),
        ]));
    frame.render_widget(Paragraph::new(lines).block(block), body);
    if state.editing.is_some() {
        footer(
            frame,
            foot,
            &[
                ("enter", "save"),
                ("esc", "cancel"),
                ("backspace", "delete"),
            ],
        );
    } else {
        footer(
            frame,
            foot,
            &[
                ("↑↓", "move"),
                ("←→/space", "change"),
                ("enter", "edit/start"),
                ("s", "start"),
                ("q", "quit"),
            ],
        );
    }
}

// ----------------------------------------------------------------- run ---

fn step_symbol(state: StepState, tick: usize) -> Span<'static> {
    match state {
        StepState::Running => Span::styled(
            SPINNER[tick % SPINNER.len()],
            Style::default().fg(Color::Cyan),
        ),
        StepState::Replayed => Span::styled("↺", dim()),
        StepState::Succeeded => Span::styled("✓", Style::default().fg(Color::Green)),
        StepState::Partial => Span::styled("◐", Style::default().fg(Color::Yellow)),
        StepState::Failed => Span::styled("✗", Style::default().fg(Color::Red)),
        StepState::Canceled => Span::styled("■", Style::default().fg(Color::Yellow)),
        StepState::AwaitingApproval => Span::styled("‖", Style::default().fg(Color::Yellow)),
        StepState::Waiting => Span::styled("…", Style::default().fg(Color::Yellow)),
    }
}

fn status_badge(view: &RunView) -> Span<'static> {
    let (text, color) = match &view.phase {
        Phase::Starting => ("starting", Color::DarkGray),
        Phase::Running if view.stopping => ("stopping", Color::Yellow),
        Phase::Running if view.is_asking() => ("needs you", Color::Yellow),
        Phase::Running => ("running", Color::Cyan),
        Phase::Finished { status, .. } => match status {
            RunStatus::Succeeded => ("done", Color::Green),
            RunStatus::Failed => ("failed", Color::Red),
            RunStatus::Canceled => ("canceled", Color::Yellow),
            RunStatus::Paused => ("paused", Color::Yellow),
            RunStatus::Rejected => ("rejected", Color::Red),
            RunStatus::AwaitingApproval => ("waiting for approval", Color::Yellow),
            RunStatus::Running => ("running", Color::Cyan),
        },
    };
    Span::styled(
        format!(" {text} "),
        Style::default()
            .fg(Color::Black)
            .bg(color)
            .add_modifier(Modifier::BOLD),
    )
}

fn bar(processed: usize, total: usize, width: usize) -> String {
    let filled = (processed * width)
        .checked_div(total)
        .unwrap_or(0)
        .min(width);
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

pub fn run(frame: &mut Frame, view: &RunView) {
    let [head, body, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let mut header = vec![
        Span::styled("codemod ", bold()),
        Span::styled(format!("· {} ", view.title), dim()),
    ];
    if let Some(id) = &view.run_id {
        // The full id is printed when the screen closes.
        let short: String = id.chars().take(8).collect();
        header.push(Span::styled(format!("· run {short} "), dim()));
    }
    if view.dry_run {
        header.push(Span::styled(
            "· dry run ",
            Style::default().fg(Color::Yellow),
        ));
    }
    if let Some(agent) = &view.agent {
        header.push(Span::styled(format!("· agent {agent} "), dim()));
    }
    if let Some(model) = &view.model {
        header.push(Span::styled(format!("· model {model} "), dim()));
    }
    let badge = status_badge(view);
    let badge_width = badge.content.chars().count() as u16;
    let [left, right] =
        Layout::horizontal([Constraint::Min(1), Constraint::Length(badge_width)]).areas(head);
    frame.render_widget(Paragraph::new(Line::from(header)), left);
    frame.render_widget(Paragraph::new(Line::from(badge)), right);

    let [steps_area, pane_area] =
        Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)]).areas(body);
    steps(frame, steps_area, view);
    pane(frame, pane_area, view);

    let mut keys: Vec<(&str, &str)> = Vec::new();
    match &view.modal {
        Some(Modal::Approval { approval, .. }) => {
            keys.push(("y", "approve"));
            keys.push(("n", "reject"));
            if matches!(approval, Approval::Manual { .. }) {
                keys.push(("l", "later"));
            }
        }
        Some(Modal::Agent { .. }) => {
            keys.extend([("↑↓", "move"), ("enter", "choose"), ("esc", "built-in")])
        }
        Some(Modal::ConfirmStop { .. }) => keys.extend([("y", "stop"), ("n", "keep running")]),
        None => {
            keys.extend([
                ("↑↓", "step"),
                ("tab", "logs/diff"),
                ("pgup/pgdn", "scroll"),
            ]);
            if view.is_finished() {
                if view.can_apply {
                    keys.push(("a", "apply for real"));
                }
                keys.push(("q", "quit"));
            } else {
                keys.push(("q", "stop"));
            }
        }
    }
    footer(frame, foot, &keys);

    match &view.modal {
        Some(Modal::Approval { approval, .. }) => approval_modal(frame, approval),
        Some(Modal::Agent {
            choices, selected, ..
        }) => {
            let mut lines = vec![
                Line::raw("A step needs a coding agent. Which one should run it?"),
                Line::raw(""),
            ];
            let rows = choices
                .iter()
                .map(|c| c.label.clone())
                .chain(std::iter::once("Built-in agent".to_string()));
            for (i, label) in rows.enumerate() {
                lines.push(if i == *selected {
                    Line::styled(format!("› {label}"), bold().fg(Color::Cyan))
                } else {
                    Line::raw(format!("  {label}"))
                });
            }
            modal(frame, " Coding agent ", lines, Color::Cyan);
        }
        Some(Modal::ConfirmStop { .. }) => modal(
            frame,
            " Stop the run? ",
            vec![
                Line::raw("The run stops and can be resumed later."),
                Line::raw(""),
                Line::from(vec![
                    Span::styled("y", bold()),
                    Span::raw(" stop   "),
                    Span::styled("n", bold()),
                    Span::raw(" keep running"),
                ]),
            ],
            Color::Yellow,
        ),
        None => {}
    }
}

fn steps(frame: &mut Frame, area: Rect, view: &RunView) {
    let width = area.width.saturating_sub(2) as usize;
    let mut lines = Vec::new();
    let changed: usize = view.diffs.len();
    let failed = view
        .steps
        .iter()
        .filter(|s| matches!(s.state, StepState::Failed | StepState::Partial))
        .count();
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut summary = plural(view.steps.len(), "step", "steps");
    if changed > 0 {
        summary.push_str(&format!(" · {}", plural(changed, "file", "files")));
    }
    if failed > 0 {
        summary.push_str(&format!(" · {} failed", plural(failed, "step", "steps")));
    }
    lines.push(selectable(
        view.selected == 0,
        vec![
            Span::styled("▣ ", Style::default().fg(Color::Blue)),
            Span::styled("Whole run ", bold()),
            Span::styled(summary, dim()),
        ],
    ));
    for (i, step) in view.steps.iter().enumerate() {
        let mut spans = vec![step_symbol(step.state, view.tick), Span::raw(" ")];
        let detail = match step.state {
            StepState::Running if step.total > 0 => format!(
                " {} {}/{}",
                bar(step.processed, step.total, 8),
                step.processed,
                step.total
            ),
            StepState::Running => match step.retry {
                Some((attempt, attempts)) => format!(" retry {}/{attempts}", attempt + 1),
                None => String::new(),
            },
            _ => {
                let mut detail = String::new();
                if step.total > 0 {
                    detail.push_str(&format!(
                        " {} file{}",
                        step.total,
                        if step.total == 1 { "" } else { "s" }
                    ));
                }
                if let Some(ms) = step.duration_ms {
                    detail.push_str(&format!(" {:.1}s", ms as f64 / 1000.0));
                }
                detail
            }
        };
        let room = width.saturating_sub(2 + detail.chars().count());
        let label: String = if step.label.chars().count() > room {
            let mut short: String = step.label.chars().take(room.saturating_sub(1)).collect();
            short.push('…');
            short
        } else {
            step.label.clone()
        };
        spans.push(Span::raw(label));
        spans.push(Span::styled(detail, dim()));
        lines.push(selectable(view.selected == i + 1, spans));
        if let Some(error) = &step.error {
            if matches!(step.state, StepState::Failed | StepState::Partial) {
                lines.push(Line::styled(
                    format!("    {error}"),
                    Style::default().fg(Color::Red),
                ));
            }
        }
    }
    // Keep the selection in view.
    let height = area.height.saturating_sub(2) as usize;
    let selected_line = lines
        .iter()
        .position(|l| l.style.add_modifier.contains(Modifier::REVERSED))
        .unwrap_or(0);
    let skip = (selected_line + 1).saturating_sub(height);
    let lines: Vec<Line> = lines.into_iter().skip(skip).collect();
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Steps ")),
        area,
    );
}

fn selectable(selected: bool, spans: Vec<Span<'static>>) -> Line<'static> {
    let line = Line::from(spans);
    if selected {
        line.style(Style::default().add_modifier(Modifier::REVERSED))
    } else {
        line
    }
}

fn diff_line(line: &str) -> Line<'static> {
    let style = if line.starts_with("+++") || line.starts_with("---") {
        bold()
    } else if line.starts_with('+') {
        Style::default().fg(Color::Green)
    } else if line.starts_with('-') {
        Style::default().fg(Color::Red)
    } else if line.starts_with("@@") {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };
    Line::styled(line.to_string(), style)
}

fn pane(frame: &mut Frame, area: Rect, view: &RunView) {
    let tab = |name: &str, on: bool| {
        if on {
            Span::styled(format!(" {name} "), bold().fg(Color::Cyan))
        } else {
            Span::styled(format!(" {name} "), dim())
        }
    };
    let scope = match view.selected_step() {
        Some(step) => step.label.clone(),
        None => "whole run".to_string(),
    };
    let title = Line::from(vec![
        tab("Logs", view.pane == Pane::Logs),
        Span::styled("│", dim()),
        tab("Diff", view.pane == Pane::Diff),
        Span::styled(format!("· {scope} "), dim()),
    ]);

    let lines: Vec<Line> = match view.pane {
        Pane::Logs => {
            let source = match view.selected_step() {
                Some(step) => &step.logs,
                None => &view.output,
            };
            let mut lines: Vec<Line> = source.iter().map(|l| Line::raw(l.clone())).collect();
            if let Some(step) = view.selected_step() {
                if let Some(error) = &step.error {
                    lines.push(Line::styled(error.clone(), Style::default().fg(Color::Red)));
                }
            }
            if let Phase::Finished {
                error: Some(error), ..
            } = &view.phase
            {
                if view.selected == 0 {
                    lines.push(Line::styled(error.clone(), Style::default().fg(Color::Red)));
                }
            }
            if lines.is_empty() {
                lines.push(Line::styled("No output yet.", dim()));
            }
            lines
        }
        Pane::Diff => {
            let diffs = view.visible_diffs();
            if diffs.is_empty() {
                let text = if view.dry_run && view.selected != 0 {
                    "A dry run shows its changes for the whole run, when it ends."
                } else if view.dry_run && !view.is_finished() {
                    "The changes show here when the dry run ends."
                } else {
                    "No changes."
                };
                vec![Line::styled(text, dim())]
            } else {
                let mut lines = Vec::new();
                for diff in diffs {
                    lines.push(Line::from(vec![
                        Span::styled(diff.path.clone(), bold()),
                        Span::styled(
                            format!(" +{}", diff.additions),
                            Style::default().fg(Color::Green),
                        ),
                        Span::styled(
                            format!(" -{}", diff.deletions),
                            Style::default().fg(Color::Red),
                        ),
                    ]));
                    lines.extend(
                        diff.diff
                            .lines()
                            .filter(|l| !l.starts_with("---") && !l.starts_with("+++"))
                            .map(diff_line),
                    );
                    lines.push(Line::raw(""));
                }
                lines
            }
        }
    };

    // Show the bottom of the text, `scroll` lines up from it; the diff
    // starts from the top instead.
    let height = area.height.saturating_sub(2) as usize;
    let total = lines.len();
    let max_offset = total.saturating_sub(height);
    let start = match view.pane {
        Pane::Logs => max_offset.saturating_sub(view.scroll),
        Pane::Diff => view.scroll.min(max_offset),
    };
    let visible: Vec<Line> = lines.into_iter().skip(start).take(height).collect();
    frame.render_widget(
        Paragraph::new(visible).block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

fn approval_modal(frame: &mut Frame, approval: &Approval) {
    let (title, text) = match approval {
        Approval::Manual { message } => (" Approval needed ", message.clone()),
        Approval::ShellCommand { command } => (" Run a shell command? ", format!("$ {command}")),
        Approval::Capabilities { modules } => (
            " Grant capabilities? ",
            format!("The transform asks to use {}.", modules.join(", ")),
        ),
        Approval::PullRequest { title, branch } => (
            " Open a pull request? ",
            format!("Push '{branch}' and open \"{title}\"."),
        ),
        Approval::UncommittedChanges {
            path,
            not_a_repository,
        } => (
            " Write over uncommitted changes? ",
            if *not_a_repository {
                format!("The next step changes files in {path}, which is not tracked by Git.")
            } else {
                format!("The next step changes files in {path}, which has uncommitted changes.")
            },
        ),
    };
    let mut keys = vec![
        Span::styled("y", bold()),
        Span::raw(" approve   "),
        Span::styled("n", bold()),
        Span::raw(" reject"),
    ];
    if matches!(approval, Approval::Manual { .. }) {
        keys.push(Span::raw("   "));
        keys.push(Span::styled("l", bold()));
        keys.push(Span::raw(" later (pause; resume with --approve)"));
    }
    modal(
        frame,
        title,
        vec![Line::raw(text), Line::raw(""), Line::from(keys)],
        Color::Yellow,
    );
}

fn modal(frame: &mut Frame, title: &str, lines: Vec<Line>, color: Color) {
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(72);
    let inner_width = width.saturating_sub(2).max(1) as usize;
    let height = lines
        .iter()
        .map(|line| line.width().div_ceil(inner_width).max(1))
        .sum::<usize>() as u16
        + 2;
    let height = height.min(area.height);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .alignment(Alignment::Left)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(color))
                    .title(Span::styled(title.to_string(), bold().fg(color))),
            ),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher::setup::{tests::input, SetupState};
    use codemod_workflow::events::{Event, StepAt};
    use codemod_workflow::StepStatus;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;

    fn text(width: u16, height: u16, draw: impl FnOnce(&mut Frame)) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(draw).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            let mut line = String::new();
            for x in 0..buffer.area.width {
                line.push_str(buffer[(x, y)].symbol());
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }

    /// Compare with `snapshots/<name>.txt`; `UPDATE_SNAPSHOTS=1` rewrites it.
    fn assert_snapshot(name: &str, actual: &str) {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/launcher/snapshots")
            .join(format!("{name}.txt"));
        if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, actual).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| {
                panic!(
                    "missing snapshot {}; run with UPDATE_SNAPSHOTS=1",
                    path.display()
                )
            })
            .replace("\r\n", "\n");
        assert_eq!(actual, expected, "frame {name} changed");
    }

    #[test]
    fn setup_frame() {
        let mut state = SetupState::new(input(PathBuf::from("/tmp")));
        state.target = "/work/demo".into();
        let frame = text(100, 24, |f| setup(f, &state));
        assert_snapshot("setup", &frame);
    }

    fn demo_run() -> RunView {
        let mut view = RunView::new("moment-to-dayjs", false);
        view.agent = Some("claude-code".into());
        view.model = Some("systemone/jev-latest".into());
        view.apply(Event::RunStarted {
            run_id: "e7470df3-ef6b-4b4b-b20b-cc00d35ddcae".into(),
            workflow: "workflow.ts".into(),
            resumed: false,
            target: "target".into(),
            params: serde_json::Value::Null,
            dry_run: false,
            settings: codemod_workflow::events::Settings {
                capabilities: Vec::new(),
                managed_git: false,
                agent: None,
                host: None,
            },
        });
        view.apply(Event::StepStarted {
            at: StepAt::top(0),
            kind: None,
            label: "Who uses moment?".into(),
        });
        view.apply(Event::RunLog {
            step: None,
            line: "moment@2.30.1".into(),
        });
        view.apply(Event::StepFinished {
            at: StepAt::top(0),
            kind: None,
            label: "Who uses moment?".into(),
            status: StepStatus::Succeeded,
            error: None,
            duration_ms: 240,
        });
        view.apply(Event::StepStarted {
            at: StepAt::top(1),
            kind: None,
            label: "Migrate packages/api".into(),
        });
        view.apply(Event::StepProgress {
            at: StepAt::top(1),
            processed: 2,
            total: 4,
        });
        view.apply(Event::StepLog {
            at: StepAt::top(1),
            line: "orders.ts: 3 calls".into(),
        });
        view
    }

    #[test]
    fn run_frame() {
        let view = demo_run();
        assert_snapshot("run", &text(100, 14, |f| run(f, &view)));
    }

    #[test]
    fn approval_frame() {
        let mut view = demo_run();
        let (reply, _answer) = std::sync::mpsc::channel();
        view.ask_approval(
            Approval::Manual {
                message: "Rewrite moment → dayjs in packages/api (4 files)?".into(),
            },
            reply,
        );
        assert_snapshot("approval", &text(100, 14, |f| run(f, &view)));
    }

    #[test]
    fn stopping_asks_first() {
        let mut view = demo_run();
        view.key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('q'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_snapshot("confirm-stop", &text(100, 14, |f| run(f, &view)));
    }

    #[test]
    fn dry_run_diff_frame() {
        let mut view = RunView::new("moment-to-dayjs", true);
        view.apply(Event::RunStarted {
            run_id: "03609be7-1c2d-4e5f-8a9b-0c1d2e3f4a5b".into(),
            workflow: "workflow.ts".into(),
            resumed: false,
            target: "target".into(),
            params: serde_json::Value::Null,
            dry_run: false,
            settings: codemod_workflow::events::Settings {
                capabilities: Vec::new(),
                managed_git: false,
                agent: None,
                host: None,
            },
        });
        view.apply(Event::StepStarted {
            at: StepAt::top(0),
            kind: None,
            label: "Rename toMoment → toDayjs".into(),
        });
        view.apply(Event::StepFinished {
            at: StepAt::top(0),
            kind: None,
            label: "Rename toMoment → toDayjs".into(),
            status: StepStatus::Failed,
            error: Some("1 file failed".into()),
            duration_ms: 1200,
        });
        view.apply(Event::RunDiff {
            path: "packages/api/src/orders.ts".into(),
            diff: "--- a/packages/api/src/orders.ts\n+++ b/packages/api/src/orders.ts\n@@ -1,2 +1,2 @@\n-import moment from \"moment\";\n+import dayjs from \"dayjs\";\n const x = 1;\n".into(),
            additions: 1,
            deletions: 1,
        });
        view.apply(Event::RunFinished {
            awaiting: None,
            question: None,
            run_id: "run-2".into(),
            status: RunStatus::Succeeded,
            error: None,
            failed_files: 0,
            partial_steps: Vec::new(),
            output: serde_json::Value::Null,
            output_omitted: None,
            steps: Vec::new(),
        });
        view.pane = Pane::Diff;
        assert_snapshot("dry-run-diff", &text(100, 14, |f| run(f, &view)));
    }
}
