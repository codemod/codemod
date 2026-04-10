use std::collections::HashSet;

use butterflow_models::WorkflowRun;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use ratatui::{
    layout::{Constraint, Layout, Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Padding},
    Frame,
};

use super::{
    key_hint, shorten_home_path, status_bar_height, truncate_middle, ACCENT, BODY_BG, DIM, GREEN,
    HEADER_BG, SURFACE, TEXT, TEXT_MUTED,
};
use super::{render_status_line, StatusLine};

/// Setting items displayed in the settings screen
const SETTINGS_COUNT: usize = 3;

/// Render the settings screen
pub fn render(
    f: &mut Frame,
    area: Rect,
    workflow_run: Option<&WorkflowRun>,
    cursor: usize,
    dry_run: bool,
    capabilities: &Option<HashSet<LlrtSupportedModules>>,
    status: Option<&StatusLine>,
) {
    let status_height = status_bar_height(status);
    let chunks = Layout::vertical([
        Constraint::Length(2),             // title bar
        Constraint::Min(0),                // content
        Constraint::Length(1),             // help bar
        Constraint::Length(status_height), // status bar
    ])
    .split(area);

    // -- Title / header bar --
    render_header(f, chunks[0], workflow_run);

    // -- Content --
    let content = chunks[1].inner(Margin::new(2, 0));
    f.render_widget(
        Block::default().style(Style::default().bg(BODY_BG)),
        chunks[1],
    );
    render_settings_list(f, content, cursor, dry_run, capabilities);

    // -- Help bar --
    render_help_bar(f, chunks[2]);
    render_status_line(f, chunks[3], status);
}

fn render_header(f: &mut Frame, area: Rect, workflow_run: Option<&WorkflowRun>) {
    let block = Block::default()
        .style(Style::default().bg(HEADER_BG))
        .padding(Padding::horizontal(1));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let name = workflow_run
        .and_then(|r| r.name.as_deref())
        .unwrap_or("Workflow");
    let target = workflow_run
        .and_then(|r| r.target_path.as_ref())
        .map(|path| shorten_home_path(path.as_path()))
        .unwrap_or_default();
    let available_width = inner.width.saturating_sub(2) as usize;

    let title_line = Line::from(vec![
        Span::styled(
            "codemod",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" / ", Style::default().fg(DIM)),
        Span::styled(
            truncate_middle(name, available_width.clamp(1, 64)),
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" / ", Style::default().fg(DIM)),
        Span::styled("session overrides", Style::default().fg(TEXT)),
    ]);
    let target_line = Line::from(Span::styled(
        truncate_middle(&format!("target: {target}"), available_width.max(1)),
        Style::default().fg(TEXT_MUTED),
    ));

    f.render_widget(title_line, Rect::new(inner.x, inner.y, inner.width, 1));
    f.render_widget(target_line, Rect::new(inner.x, inner.y + 1, inner.width, 1));
}

fn is_capability_on(
    capabilities: &Option<HashSet<LlrtSupportedModules>>,
    module: LlrtSupportedModules,
) -> bool {
    match capabilities {
        None => false, // no capabilities granted
        Some(set) => set.contains(&module),
    }
}

struct SettingItem {
    label: &'static str,
    description: &'static str,
    enabled: bool,
}

fn render_settings_list(
    f: &mut Frame,
    area: Rect,
    cursor: usize,
    dry_run: bool,
    capabilities: &Option<HashSet<LlrtSupportedModules>>,
) {
    let items = [
        SettingItem {
            label: "Dry run",
            description: "Preview changes without writing to disk for this TUI session",
            enabled: dry_run,
        },
        SettingItem {
            label: "Capability: fs",
            description: "Allow filesystem access for tasks triggered from this TUI session",
            enabled: is_capability_on(capabilities, LlrtSupportedModules::Fs),
        },
        SettingItem {
            label: "Capability: fetch",
            description: "Allow network requests for tasks triggered from this TUI session",
            enabled: is_capability_on(capabilities, LlrtSupportedModules::Fetch),
        },
    ];

    // Each item takes 2 lines (label + description) + 1 line gap
    let mut y = area.y;
    for (i, item) in items.iter().enumerate() {
        if y + 1 >= area.y + area.height {
            break;
        }

        let is_selected = i == cursor;
        let icon = if item.enabled { "\u{25c9}" } else { "\u{25cb}" }; // ◉ vs ○
        let icon_color = if item.enabled { GREEN } else { DIM };

        let cursor_indicator = if is_selected { "\u{25b8} " } else { "  " }; // ▸ vs space

        let row_bg = if is_selected {
            Style::default().bg(SURFACE)
        } else {
            Style::default()
        };

        // Label line
        let label_line = Line::from(vec![
            Span::styled(cursor_indicator, Style::default().fg(ACCENT)),
            Span::styled(format!("{icon} "), Style::default().fg(icon_color)),
            Span::styled(
                item.label,
                Style::default().fg(TEXT).add_modifier(if is_selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
        ]);

        let label_area = Rect::new(area.x, y, area.width, 1);
        f.render_widget(
            Block::default().style(row_bg),
            Rect::new(area.x, y, area.width, 2),
        );
        f.render_widget(label_line, label_area);

        // Description line
        if y + 1 < area.y + area.height {
            let desc_line = Line::from(Span::styled(
                format!("    {}", item.description),
                Style::default().fg(TEXT_MUTED),
            ));
            let desc_area = Rect::new(area.x, y + 1, area.width, 1);
            f.render_widget(desc_line, desc_area);
        }

        y += 3; // 2 lines content + 1 line gap
    }
}

fn render_help_bar(f: &mut Frame, area: Rect) {
    f.render_widget(Block::default().style(Style::default().bg(BODY_BG)), area);
    let padded = area.inner(Margin::new(1, 0));
    let mut spans: Vec<Span> = Vec::new();
    spans.extend(key_hint("\u{2191}\u{2193}", "navigate"));
    spans.extend(key_hint("\u{23ce}", "toggle"));
    spans.extend(key_hint("esc", "back"));
    spans.extend(key_hint("q", "quit"));

    f.render_widget(Line::from(spans), padded);
}

/// Total number of setting items
pub const fn settings_count() -> usize {
    SETTINGS_COUNT
}
