//! Python manifests: `pyproject.toml` (PEP 621 `[project]`, PEP 735
//! `[dependency-groups]`, uv's `[tool.uv] dev-dependencies`, Poetry's
//! `[tool.poetry.*dependencies]`) and `requirements*.txt`. Read and edited
//! as text, line by line, so formatting, comments and markers stay.
//!
//! Kinds: `[project] dependencies`, Poetry's main group and a plain
//! requirements file are `prod`; dependency groups, Poetry groups, uv dev
//! dependencies, optional-dependency groups named like dev/test/lint/docs,
//! and requirements files named like dev/test are `dev`; other
//! optional-dependency groups (extras) are `optional`.

use std::ops::Range;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::manifest_edits::Edit;
use crate::model::Kind;

/// Whether a path is a Python manifest this module reads.
pub fn is_manifest(path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    file == "pyproject.toml" || is_requirements(file)
}

fn is_requirements(file: &str) -> bool {
    file.ends_with(".txt") && (file.starts_with("requirements") || file.contains("requirements"))
}

/// Every Python manifest under `root`, relative with `/`: `.gitignore` is
/// respected; virtualenvs and `node_modules` are skipped. Files under a
/// `requirements/` directory count too.
pub fn discover(root: &Path) -> Vec<String> {
    let mut found: Vec<String> = ignore::WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|entry| {
            !matches!(
                entry.file_name().to_str(),
                Some(
                    "node_modules"
                        | ".git"
                        | "target"
                        | ".venv"
                        | "venv"
                        | ".tox"
                        | "__pypackages__"
                        | "site-packages"
                )
            )
        })
        .build()
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|entry| {
            let relative = entry.path().strip_prefix(root).ok()?;
            let relative = relative.to_string_lossy().replace('\\', "/");
            let in_requirements_dir = relative
                .rsplit('/')
                .nth(1)
                .is_some_and(|dir| dir == "requirements")
                && relative.ends_with(".txt");
            (is_manifest(&relative) || in_requirements_dir).then_some(relative)
        })
        .collect();
    found.sort_by(|a, b| {
        a.matches('/')
            .count()
            .cmp(&b.matches('/').count())
            .then((!a.ends_with("pyproject.toml")).cmp(&!b.ends_with("pyproject.toml")))
            .then(a.cmp(b))
    });
    found
}

/// PEP 503 normalized name.
pub fn normalize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut dash = false;
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            dash = true;
        } else {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.extend(c.to_lowercase());
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    /// A PEP 508 string: a requirements line or an item of a TOML array.
    Pep508,
    /// Poetry: `name = "spec"` or `name = { version = "spec", ... }`.
    Poetry,
}

/// One declaration found in a manifest's text.
#[derive(Debug, Clone)]
struct Found {
    name: String,
    spec: String,
    kind: Kind,
    group: Option<String>,
    style: Style,
    name_span: Range<usize>,
    /// The spec's text; empty at the insertion point when there is none.
    spec_span: Range<usize>,
    /// What `remove` deletes.
    remove_span: Range<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PyDeclared {
    pub name: String,
    /// As written (PEP 440 specifier, or Poetry's constraint).
    pub range: String,
    pub kind: Kind,
    /// The optional-dependency, dependency or Poetry group.
    pub group: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PyManifest {
    pub dir: String,
    pub manifest: String,
    pub name: Option<String>,
    pub version: Option<String>,
    pub declarations: Vec<PyDeclared>,
}

fn dir_of(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => ".".to_string(),
    }
}

/// Read one manifest.
pub fn read(path: &str, text: &str) -> PyManifest {
    let found = scan(path, text);
    let (name, version) = if path.ends_with(".toml") {
        project_name(text)
    } else {
        (None, None)
    };
    PyManifest {
        dir: dir_of(path),
        manifest: path.to_string(),
        name,
        version,
        declarations: found
            .into_iter()
            .map(|f| PyDeclared {
                name: f.name,
                range: f.spec,
                kind: f.kind,
                group: f.group,
            })
            .collect(),
    }
}

fn project_name(text: &str) -> (Option<String>, Option<String>) {
    let Ok(value) = text.parse::<toml::Table>() else {
        return (None, None);
    };
    let pick = |table: Option<&toml::Value>, key: &str| {
        table
            .and_then(|t| t.get(key))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let project = value.get("project");
    let poetry = value.get("tool").and_then(|t| t.get("poetry"));
    (
        pick(project, "name").or_else(|| pick(poetry, "name")),
        pick(project, "version").or_else(|| pick(poetry, "version")),
    )
}

fn group_kind(group: &str) -> Kind {
    let g = normalize(group);
    if [
        "dev",
        "test",
        "tests",
        "testing",
        "lint",
        "linting",
        "docs",
        "doc",
        "typing",
        "types",
        "develop",
        "development",
        "ci",
    ]
    .contains(&g.as_str())
        || g.starts_with("dev")
        || g.starts_with("test")
    {
        Kind::Dev
    } else {
        Kind::Optional
    }
}

/// Split a PEP 508 requirement at `offset` in the text: its name, and its
/// spec (between the extras and a `;` marker).
fn pep508(text: &str, start: usize, end: usize) -> Option<(Range<usize>, Range<usize>)> {
    let s = &text[start..end];
    let lead = s.len() - s.trim_start().len();
    let rest = &s[lead..];
    let name_len = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
        .unwrap_or(rest.len());
    if name_len == 0 {
        return None;
    }
    let name = start + lead..start + lead + name_len;
    let mut i = lead + name_len;
    let after = &s[i..];
    let ws = after.len() - after.trim_start().len();
    if after[ws..].starts_with('[') {
        let close = after[ws..].find(']')?;
        i += ws + close + 1;
    }
    let marker = s[i..].find(';').map(|m| i + m).unwrap_or(s.len());
    let spec_text = &s[i..marker];
    let spec_lead = spec_text.len() - spec_text.trim_start().len();
    let spec_trim = spec_text.trim();
    if spec_trim.starts_with('@') || spec_trim.contains("://") {
        // A URL requirement: no spec to edit.
        return None;
    }
    let spec_start = if spec_trim.is_empty() {
        i
    } else {
        i + spec_lead
    };
    let spec = start + spec_start..start + spec_start + spec_trim.len();
    Some((name, spec))
}

fn scan(path: &str, text: &str) -> Vec<Found> {
    if path.ends_with(".toml") {
        scan_pyproject(text)
    } else {
        scan_requirements(path, text)
    }
}

/// Lines with their start offsets, the newline included in the range.
fn lines(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        out.push((start, line));
        start += line.len();
    }
    out
}

fn scan_requirements(path: &str, text: &str) -> Vec<Found> {
    let file = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    let kind = if file.contains("dev") || file.contains("test") || file.contains("lint") {
        Kind::Dev
    } else {
        Kind::Prod
    };
    let mut out = Vec::new();
    for (start, line) in lines(text) {
        let body = line.trim_end_matches(['\n', '\r']);
        let trimmed = body.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
            continue;
        }
        let end = body
            .find(" #")
            .or_else(|| body.find("\t#"))
            .unwrap_or(body.len());
        let Some((name, spec)) = pep508(text, start, start + end) else {
            continue;
        };
        out.push(Found {
            name: text[name.clone()].to_string(),
            spec: text[spec.clone()].to_string(),
            kind,
            group: None,
            style: Style::Pep508,
            name_span: name,
            spec_span: spec,
            remove_span: start..start + line.len(),
        });
    }
    out
}

/// The kind of a PEP 508 array at `path` (table + key), if it holds dependencies.
fn array_kind(path: &str) -> Option<(Kind, Option<String>)> {
    if path == "project.dependencies" {
        return Some((Kind::Prod, None));
    }
    if let Some(group) = path.strip_prefix("project.optional-dependencies.") {
        return Some((group_kind(group), Some(group.to_string())));
    }
    if let Some(group) = path.strip_prefix("dependency-groups.") {
        return Some((Kind::Dev, Some(group.to_string())));
    }
    if path == "tool.uv.dev-dependencies" {
        return Some((Kind::Dev, Some("dev".to_string())));
    }
    None
}

/// The kind of a Poetry dependency table.
fn poetry_kind(table: &str) -> Option<(Kind, Option<String>)> {
    if table == "tool.poetry.dependencies" {
        return Some((Kind::Prod, None));
    }
    if table == "tool.poetry.dev-dependencies" {
        return Some((Kind::Dev, Some("dev".to_string())));
    }
    let group = table
        .strip_prefix("tool.poetry.group.")?
        .strip_suffix(".dependencies")?;
    Some((Kind::Dev, Some(group.to_string())))
}

fn unquote_key(key: &str) -> String {
    key.split('.')
        .map(|part| part.trim().trim_matches('"').trim_matches('\''))
        .collect::<Vec<_>>()
        .join(".")
}

/// Quoted strings on a line before a comment, as content ranges (absolute),
/// each with whether it is a value (`= "..."`) rather than an item.
fn strings(text: &str, start: usize, line: &str) -> (Vec<(Range<usize>, bool)>, Option<usize>) {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut close = None;
    let mut i = 0;
    let mut last_sig: u8 = b' ';
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'#' => break,
            b'"' | b'\'' => {
                let quote = c;
                let mut j = i + 1;
                while j < bytes.len() && bytes[j] != quote {
                    if bytes[j] == b'\\' {
                        j += 1;
                    }
                    j += 1;
                }
                if j >= bytes.len() {
                    break;
                }
                out.push((start + i + 1..start + j, last_sig == b'='));
                last_sig = quote;
                i = j + 1;
                continue;
            }
            b']' => {
                close = Some(start + i);
                last_sig = c;
            }
            b' ' | b'\t' | b'\r' | b'\n' => {}
            _ => last_sig = c,
        }
        i += 1;
    }
    let _ = text;
    (out, close)
}

fn scan_pyproject(text: &str) -> Vec<Found> {
    let mut out = Vec::new();
    let mut table = String::new();
    // Inside a multi-line array: its key path.
    let mut array: Option<String> = None;
    for (start, line) in lines(text) {
        let trimmed = line.trim();
        if array.is_none() && trimmed.starts_with('[') {
            let inner = trimmed
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or("");
            table = unquote_key(inner);
            continue;
        }
        if let Some(path) = array.clone() {
            let (found, close) = strings(text, start, line);
            for (range, value) in found {
                if close.is_some_and(|c| range.start > c) || value {
                    continue;
                }
                push_pep508(&mut out, text, &path, range, start, line);
            }
            if close.is_some() {
                array = None;
            }
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some(eq) = line.find('=') else { continue };
        let key = unquote_key(&line[..eq]);
        let value = line[eq + 1..].trim_start();
        let path = if table.is_empty() {
            key.clone()
        } else {
            format!("{table}.{key}")
        };
        if value.starts_with('[') {
            if array_kind(&path).is_none() {
                // Skip to the end of an array we don't read.
                let (_, close) = strings(text, start, line);
                if close.is_none() {
                    array = Some(String::new());
                }
                continue;
            }
            let open = start + eq + 1 + (line[eq + 1..].len() - value.len());
            let rest = &text[open + 1..start + line.len()];
            let (found, close) = strings(text, open + 1, rest);
            for (range, value) in found {
                if close.is_some_and(|c| range.start > c) || value {
                    continue;
                }
                push_pep508(&mut out, text, &path, range, start, line);
            }
            if close.is_none() {
                array = Some(path);
            }
            continue;
        }
        if let Some((kind, group)) = poetry_kind(&table) {
            if key == "python" {
                continue;
            }
            let name_start = start + (line.len() - line.trim_start().len());
            let raw_key = line[..eq].trim();
            let key_off = usize::from(raw_key.starts_with('"') || raw_key.starts_with('\''));
            let name_span = name_start + key_off..name_start + key_off + key.len();
            let (found, _) = strings(text, start, line);
            // `name = "spec"`, or the string after `version =` in an inline table.
            let spec = if value.starts_with('{') {
                found.into_iter().find(|(range, _)| {
                    let before = text[start..range.start - 1].trim_end();
                    before.ends_with('=')
                        && before[..before.len() - 1].trim_end().ends_with("version")
                })
            } else {
                found.into_iter().next()
            };
            let Some((spec, _)) = spec else { continue };
            out.push(Found {
                name: key,
                spec: text[spec.clone()].to_string(),
                kind,
                group,
                style: Style::Poetry,
                name_span,
                spec_span: spec,
                remove_span: start..start + line.len(),
            });
        }
    }
    out
}

fn push_pep508(
    out: &mut Vec<Found>,
    text: &str,
    path: &str,
    content: Range<usize>,
    line_start: usize,
    line: &str,
) {
    if path.is_empty() {
        return;
    }
    let Some((kind, group)) = array_kind(path) else {
        return;
    };
    let Some((name, spec)) = pep508(text, content.start, content.end) else {
        return;
    };
    // Remove the item with its comma; the whole line when it is alone there.
    let quote_start = content.start - 1;
    let mut end = content.end + 1;
    let after = &text[end..];
    let ws = after.len() - after.trim_start_matches([' ', '\t']).len();
    let trailing_comma = after[ws..].starts_with(',');
    if trailing_comma {
        end += ws + 1;
    }
    let line_end = line_start + line.len();
    let before = &text[line_start..quote_start];
    let after_item = &text[end..line_end];
    let remove_span = if before.trim().is_empty()
        && (after_item.trim().is_empty() || after_item.trim().starts_with('#'))
    {
        line_start..line_end
    } else {
        // Inline: also drop the spaces after the comma; the last item takes
        // the comma before it.
        let more = text[end..]
            .len()
            .saturating_sub(text[end..].trim_start_matches([' ', '\t']).len());
        let before_trim = text[line_start..quote_start].trim_end();
        if !trailing_comma && before_trim.ends_with(',') {
            line_start + before_trim.len() - 1..end
        } else {
            quote_start..(end + more).min(line_end)
        }
    };
    out.push(Found {
        name: text[name.clone()].to_string(),
        spec: text[spec.clone()].to_string(),
        kind,
        group,
        style: Style::Pep508,
        name_span: name,
        spec_span: spec,
        remove_span,
    });
}

/// A range for a PEP 508 string: a bare version pins it, and Poetry/npm
/// style `^` and `~` become their PEP 440 bounds.
pub fn to_pep440(range: &str) -> String {
    let r = range.trim();
    let nums = |v: &str| -> Vec<u64> {
        v.split('.')
            .map(|p| p.trim().parse::<u64>().unwrap_or(0))
            .collect()
    };
    if let Some(v) = r.strip_prefix('^') {
        let n = nums(v);
        let upper = if n.first().copied().unwrap_or(0) > 0 || n.len() == 1 {
            format!("{}", n[0] + 1)
        } else if n.get(1).copied().unwrap_or(0) > 0 || n.len() == 2 {
            format!("0.{}", n.get(1).copied().unwrap_or(0) + 1)
        } else {
            format!("0.0.{}", n.get(2).copied().unwrap_or(0) + 1)
        };
        return format!(">={v},<{upper}");
    }
    if let Some(v) = r.strip_prefix('~').filter(|v| !v.starts_with('=')) {
        let n = nums(v);
        let upper = if n.len() >= 2 {
            format!("{}.{}", n[0], n[1] + 1)
        } else {
            format!("{}", n[0] + 1)
        };
        return format!(">={v},<{upper}");
    }
    if r.starts_with(|c: char| c.is_ascii_digit()) {
        return format!("=={r}");
    }
    if r == "*" {
        return String::new();
    }
    r.to_string()
}

/// One requested change to a Python manifest (`manifest` names the file).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum PyChange {
    Set {
        manifest: String,
        kind: Kind,
        name: String,
        range: String,
    },
    Add {
        manifest: String,
        kind: Kind,
        name: String,
        range: String,
    },
    Remove {
        manifest: String,
        kind: Kind,
        name: String,
    },
    Rename {
        manifest: String,
        kind: Kind,
        from: String,
        to: String,
        #[serde(default)]
        range: Option<String>,
    },
}

impl PyChange {
    pub fn manifest(&self) -> &str {
        match self {
            Self::Set { manifest, .. }
            | Self::Add { manifest, .. }
            | Self::Remove { manifest, .. }
            | Self::Rename { manifest, .. } => manifest,
        }
    }
}

fn spec_for(style: Style, range: &str) -> String {
    match style {
        Style::Pep508 => to_pep440(range),
        Style::Poetry => range.trim().to_string(),
    }
}

fn splice(text: &str, span: Range<usize>, with: &str) -> String {
    format!("{}{}{}", &text[..span.start], with, &text[span.end..])
}

/// Apply one change to a manifest's text; the new text and what changed.
pub fn apply(path: &str, text: &str, change: &PyChange) -> Result<(String, Vec<Edit>), String> {
    let dir = dir_of(path);
    let edit = |name: &str,
                kind: Kind,
                from: Option<String>,
                to: Option<String>,
                renamed: Option<String>| Edit {
        file: path.to_string(),
        package: Some(dir.clone()),
        name: name.to_string(),
        kind: kind.as_str().to_string(),
        from,
        to,
        renamed_from: renamed,
    };
    let matching = |name: &str, kind: Kind| -> Vec<Found> {
        let wanted = normalize(name);
        scan(path, text)
            .into_iter()
            .filter(|f| normalize(&f.name) == wanted && f.kind == kind)
            .collect()
    };
    match change {
        PyChange::Set {
            kind, name, range, ..
        } => {
            let mut out = text.to_string();
            let mut edits = Vec::new();
            // Back to front, so earlier spans stay valid.
            let mut found = matching(name, *kind);
            found.sort_by_key(|f| std::cmp::Reverse(f.spec_span.start));
            for f in found {
                let spec = spec_for(f.style, range);
                if spec == f.spec {
                    continue;
                }
                out = splice(&out, f.spec_span.clone(), &spec);
                edits.push(edit(&f.name, *kind, Some(f.spec.clone()), Some(spec), None));
            }
            edits.reverse();
            Ok((out, edits))
        }
        PyChange::Remove { kind, name, .. } => {
            let mut out = text.to_string();
            let mut edits = Vec::new();
            let mut found = matching(name, *kind);
            found.sort_by_key(|f| std::cmp::Reverse(f.remove_span.start));
            for f in found {
                out = splice(&out, f.remove_span.clone(), "");
                edits.push(edit(&f.name, *kind, Some(f.spec.clone()), None, None));
            }
            edits.reverse();
            Ok((out, edits))
        }
        PyChange::Rename {
            kind,
            from,
            to,
            range,
            ..
        } => {
            let mut out = text.to_string();
            let mut edits = Vec::new();
            let mut found = matching(from, *kind);
            found.sort_by_key(|f| std::cmp::Reverse(f.name_span.start));
            for f in found {
                let spec = range
                    .as_deref()
                    .map(|r| spec_for(f.style, r))
                    .unwrap_or_else(|| f.spec.clone());
                // spec comes after the name: splice it first.
                if spec != f.spec {
                    out = splice(&out, f.spec_span.clone(), &spec);
                }
                out = splice(&out, f.name_span.clone(), to);
                edits.push(edit(
                    to,
                    *kind,
                    Some(f.spec.clone()),
                    Some(spec),
                    Some(f.name.clone()),
                ));
            }
            edits.reverse();
            Ok((out, edits))
        }
        PyChange::Add {
            kind, name, range, ..
        } => {
            if !matching(name, *kind).is_empty() {
                let set = PyChange::Set {
                    manifest: path.to_string(),
                    kind: *kind,
                    name: name.clone(),
                    range: range.clone(),
                };
                return apply(path, text, &set);
            }
            let out = add(path, text, *kind, name, range)?;
            let style = if poetry_table_for(text, *kind).is_some() && !has_pep621(text) {
                Style::Poetry
            } else {
                Style::Pep508
            };
            Ok((
                out,
                vec![edit(name, *kind, None, Some(spec_for(style, range)), None)],
            ))
        }
    }
}

fn has_pep621(text: &str) -> bool {
    text.lines().any(|l| l.trim() == "[project]")
}

fn poetry_table_for(text: &str, kind: Kind) -> Option<String> {
    let has = |t: &str| text.lines().any(|l| l.trim() == format!("[{t}]"));
    match kind {
        Kind::Prod if has("tool.poetry.dependencies") => Some("tool.poetry.dependencies".into()),
        Kind::Dev if has("tool.poetry.group.dev.dependencies") => {
            Some("tool.poetry.group.dev.dependencies".into())
        }
        Kind::Dev if has("tool.poetry.dev-dependencies") => {
            Some("tool.poetry.dev-dependencies".into())
        }
        Kind::Dev if has("tool.poetry.dependencies") => {
            Some("tool.poetry.group.dev.dependencies".into())
        }
        _ => None,
    }
}

/// Insert an item at the end of the array that starts at `key_line` (the
/// offset of its `key = [` line).
fn append_to_array(text: &str, key_line: usize, item: &str) -> String {
    let open = key_line + text[key_line..].find('[').unwrap_or(0);
    // Find the matching `]`, skipping strings and comments.
    let bytes = text.as_bytes();
    let mut i = open + 1;
    let mut close = None;
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' => {
                let q = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != q {
                    i += 1;
                }
            }
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b']' => {
                close = Some(i);
                break;
            }
            _ => {}
        }
        i += 1;
    }
    let Some(close) = close else {
        return text.to_string();
    };
    let inner = &text[open + 1..close];
    if inner.contains('\n') {
        // Multi-line: a new line with the last item's indentation.
        let indent = inner
            .lines()
            .rev()
            .find(|l| l.trim_start().starts_with(['"', '\'']))
            .map(|l| &l[..l.len() - l.trim_start().len()])
            .unwrap_or("    ")
            .to_string();
        let trimmed_end = text[..close].trim_end().len();
        let needs_comma = !text[open + 1..trimmed_end].trim().is_empty()
            && !text[..trimmed_end].ends_with(',')
            && !text[open + 1..trimmed_end].trim().ends_with('[');
        let line_start = text[..close].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let rest = if text[line_start..close].trim().is_empty() {
            &text[line_start..]
        } else {
            &text[close..]
        };
        format!(
            "{}{}\n{indent}\"{item}\",\n{rest}",
            &text[..trimmed_end],
            if needs_comma { "," } else { "" },
        )
    } else if inner.trim().is_empty() {
        format!("{}\"{item}\"{}", &text[..open + 1], &text[close..])
    } else {
        let body = inner.trim_end();
        let comma = if body.ends_with(',') { " " } else { ", " };
        format!(
            "{}{}{comma}\"{item}\"{}",
            &text[..open + 1],
            body,
            &text[close..]
        )
    }
}

/// The offset of the line `key = [` in `table`, if any.
fn find_array(text: &str, table: &str, key: &str) -> Option<usize> {
    let mut current = String::new();
    for (start, line) in lines(text) {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && !trimmed.starts_with("[[") {
            let inner = trimmed
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or("");
            current = unquote_key(inner);
            continue;
        }
        if current == table {
            if let Some(eq) = line.find('=') {
                if unquote_key(&line[..eq]) == key && line[eq + 1..].trim_start().starts_with('[') {
                    return Some(start);
                }
            }
        }
    }
    None
}

/// The end of a table's last non-blank line, for inserting a key.
fn table_end(text: &str, table: &str) -> Option<usize> {
    let mut inside = false;
    let mut end = None;
    for (start, line) in lines(text) {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if inside {
                break;
            }
            let inner = trimmed
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or("");
            inside = unquote_key(inner) == table;
            if inside {
                end = Some(start + line.len());
            }
            continue;
        }
        if inside && !trimmed.is_empty() {
            end = Some(start + line.len());
        }
    }
    end
}

fn add(path: &str, text: &str, kind: Kind, name: &str, range: &str) -> Result<String, String> {
    if !path.ends_with(".toml") {
        let mut out = text.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("{name}{}\n", to_pep440(range)));
        return Ok(out);
    }
    let item = format!("{name}{}", to_pep440(range));
    if has_pep621(text) {
        let spots: &[(&str, &str)] = match kind {
            Kind::Prod => &[("project", "dependencies")],
            Kind::Dev => &[
                ("dependency-groups", "dev"),
                ("tool.uv", "dev-dependencies"),
                ("project.optional-dependencies", "dev"),
                ("project.optional-dependencies", "test"),
            ],
            Kind::Optional => &[],
            _ => &[],
        };
        for (table, key) in spots {
            if let Some(line) = find_array(text, table, key) {
                return Ok(append_to_array(text, line, &item));
            }
        }
        return Ok(match kind {
            Kind::Prod => {
                let end = table_end(text, "project").ok_or("no [project] table")?;
                format!(
                    "{}dependencies = [\n    \"{item}\",\n]\n{}",
                    &text[..end],
                    &text[end..]
                )
            }
            Kind::Dev => {
                let mut out = text.trim_end().to_string();
                out.push_str(&format!(
                    "\n\n[dependency-groups]\ndev = [\n    \"{item}\",\n]\n"
                ));
                out
            }
            other => {
                return Err(format!(
                    "add: kind '{}' is not supported for {path}",
                    other.as_str()
                ))
            }
        });
    }
    let table = poetry_table_for(text, kind)
        .ok_or_else(|| format!("add: {path} has no [project] or Poetry dependency table"))?;
    let line = format!("{name} = \"{}\"\n", range.trim());
    Ok(match table_end(text, &table) {
        Some(end) => {
            let mut prefix = text[..end].to_string();
            if !prefix.ends_with('\n') {
                prefix.push('\n');
            }
            format!("{prefix}{line}{}", &text[end..])
        }
        None => {
            let mut out = text.trim_end().to_string();
            out.push_str(&format!("\n\n[{table}]\n{line}"));
            out
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEP621: &str = r#"[project]
name = "app"
version = "1.0.0"
dependencies = [
    "pydantic>=1.10,<2",  # pinned
    "fastapi[all] >= 0.95 ; python_version >= '3.9'",
    "requests",
]

[project.optional-dependencies]
test = ["pytest>=7", "pytest-cov"]
s3 = ["boto3>=1.26"]

[dependency-groups]
dev = [
    "ruff>=0.1",
    { include-group = "test" },
]
"#;

    const POETRY: &str = r#"[tool.poetry]
name = "svc"
version = "0.1.0"

[tool.poetry.dependencies]
python = "^3.10"
Django = "^3.2"
celery = { version = "^5.2", extras = ["redis"] }

[tool.poetry.group.dev.dependencies]
pytest = "^7.0"
"#;

    fn decls(path: &str, text: &str) -> Vec<(String, String, Kind)> {
        read(path, text)
            .declarations
            .into_iter()
            .map(|d| (d.name, d.range, d.kind))
            .collect()
    }

    #[test]
    fn reads_pep621_groups_and_extras() {
        let found = decls("pyproject.toml", PEP621);
        assert_eq!(
            found,
            vec![
                ("pydantic".into(), ">=1.10,<2".into(), Kind::Prod),
                ("fastapi".into(), ">= 0.95".into(), Kind::Prod),
                ("requests".into(), "".into(), Kind::Prod),
                ("pytest".into(), ">=7".into(), Kind::Dev),
                ("pytest-cov".into(), "".into(), Kind::Dev),
                ("boto3".into(), ">=1.26".into(), Kind::Optional),
                ("ruff".into(), ">=0.1".into(), Kind::Dev),
            ]
        );
        let m = read("pyproject.toml", PEP621);
        assert_eq!(m.name.as_deref(), Some("app"));
    }

    #[test]
    fn reads_poetry() {
        assert_eq!(
            decls("pyproject.toml", POETRY),
            vec![
                ("Django".into(), "^3.2".into(), Kind::Prod),
                ("celery".into(), "^5.2".into(), Kind::Prod),
                ("pytest".into(), "^7.0".into(), Kind::Dev),
            ]
        );
    }

    fn set(path: &str, text: &str, kind: Kind, name: &str, range: &str) -> String {
        apply(
            path,
            text,
            &PyChange::Set {
                manifest: path.into(),
                kind,
                name: name.into(),
                range: range.into(),
            },
        )
        .unwrap()
        .0
    }

    #[test]
    fn bumps_keep_extras_markers_and_comments() {
        let out = set("pyproject.toml", PEP621, Kind::Prod, "pydantic", ">=2.5,<3");
        assert!(out.contains("\"pydantic>=2.5,<3\",  # pinned"), "{out}");
        let out = set("pyproject.toml", PEP621, Kind::Prod, "FastAPI", "^0.110");
        assert!(
            out.contains("\"fastapi[all] >=0.110,<0.111 ; python_version >= '3.9'\""),
            "{out}"
        );
        let out = set("pyproject.toml", PEP621, Kind::Prod, "requests", "2.31.0");
        assert!(out.contains("\"requests==2.31.0\""), "{out}");
        let out = set("pyproject.toml", POETRY, Kind::Prod, "django", "^4.2");
        assert!(out.contains("Django = \"^4.2\""), "{out}");
        let out = set("pyproject.toml", POETRY, Kind::Prod, "celery", "^5.3");
        assert!(
            out.contains("celery = { version = \"^5.3\", extras"),
            "{out}"
        );
    }

    #[test]
    fn requirements_files() {
        let text = "# deps\nDjango==3.2.18  # lts\npydantic>=1.10\n-r base.txt\n";
        assert_eq!(
            decls("requirements.txt", text),
            vec![
                ("Django".into(), "==3.2.18".into(), Kind::Prod),
                ("pydantic".into(), ">=1.10".into(), Kind::Prod),
            ]
        );
        let out = set("requirements.txt", text, Kind::Prod, "django", "4.2.11");
        assert_eq!(
            out,
            "# deps\nDjango==4.2.11  # lts\npydantic>=1.10\n-r base.txt\n"
        );
        let (out, _) = apply(
            "requirements.txt",
            text,
            &PyChange::Remove {
                manifest: "requirements.txt".into(),
                kind: Kind::Prod,
                name: "pydantic".into(),
            },
        )
        .unwrap();
        assert_eq!(out, "# deps\nDjango==3.2.18  # lts\n-r base.txt\n");
        let (out, _) = apply(
            "requirements-dev.txt",
            "pytest\n",
            &PyChange::Add {
                manifest: "requirements-dev.txt".into(),
                kind: Kind::Dev,
                name: "pytest-django".into(),
                range: ">=4".into(),
            },
        )
        .unwrap();
        assert_eq!(out, "pytest\npytest-django>=4\n");
    }

    #[test]
    fn add_remove_rename_in_pyproject() {
        let add = |text: &str, kind, name: &str, range: &str| {
            apply(
                "pyproject.toml",
                text,
                &PyChange::Add {
                    manifest: "pyproject.toml".into(),
                    kind,
                    name: name.into(),
                    range: range.into(),
                },
            )
            .unwrap()
            .0
        };
        let out = add(PEP621, Kind::Prod, "pydantic-settings", ">=2");
        assert!(
            out.contains("    \"requests\",\n    \"pydantic-settings>=2\",\n]"),
            "{out}"
        );
        assert_eq!(decls("pyproject.toml", &out).len(), 8);
        let out = add(PEP621, Kind::Dev, "mypy", ">=1");
        assert!(
            decls("pyproject.toml", &out).contains(&("mypy".into(), ">=1".into(), Kind::Dev)),
            "{out}"
        );
        let out = add(POETRY, Kind::Prod, "redis", "^5.0");
        assert!(
            out.contains(
                "celery = { version = \"^5.2\", extras = [\"redis\"] }\nredis = \"^5.0\"\n"
            ),
            "{out}"
        );

        let (out, _) = apply(
            "pyproject.toml",
            PEP621,
            &PyChange::Remove {
                manifest: "pyproject.toml".into(),
                kind: Kind::Dev,
                name: "pytest".into(),
            },
        )
        .unwrap();
        assert!(out.contains("test = [\"pytest-cov\"]"), "{out}");
        let (out, _) = apply(
            "pyproject.toml",
            PEP621,
            &PyChange::Remove {
                manifest: "pyproject.toml".into(),
                kind: Kind::Prod,
                name: "requests".into(),
            },
        )
        .unwrap();
        assert!(!out.contains("requests"), "{out}");
        assert!(out.contains("python_version >= '3.9'\",\n]"), "{out}");

        let (out, _) = apply(
            "pyproject.toml",
            PEP621,
            &PyChange::Rename {
                manifest: "pyproject.toml".into(),
                kind: Kind::Prod,
                from: "pydantic".into(),
                to: "pydantic-core".into(),
                range: Some(">=2".into()),
            },
        )
        .unwrap();
        assert!(out.contains("\"pydantic-core>=2\",  # pinned"), "{out}");
    }

    #[test]
    fn pep440_conversion() {
        assert_eq!(to_pep440("^2.5"), ">=2.5,<3");
        assert_eq!(to_pep440("^0.110"), ">=0.110,<0.111");
        assert_eq!(to_pep440("~4.2"), ">=4.2,<4.3");
        assert_eq!(to_pep440("4.2.11"), "==4.2.11");
        assert_eq!(to_pep440(">=2,<3"), ">=2,<3");
        assert_eq!(to_pep440("~=4.2"), "~=4.2");
    }
}
