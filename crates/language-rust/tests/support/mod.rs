//! A small Cargo project on disk, and short ways to ask about it.
#![allow(dead_code)]

use language_core::{
    ByteRange, CallGraphDirection, CallGraphOptions, FlowDirection, FlowOptions, SemanticProvider,
};
use language_rust::RaSemanticProvider;
use std::path::PathBuf;
use tempfile::TempDir;

const MANIFEST: &str = "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

pub struct Project {
    _dir: TempDir,
    root: PathBuf,
    pub provider: RaSemanticProvider,
}

impl Project {
    /// A crate named `shop` with these files (a `Cargo.toml` of your own
    /// replaces the default one).
    pub fn new(files: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        if !files.iter().any(|(path, _)| *path == "Cargo.toml") {
            std::fs::write(root.join("Cargo.toml"), MANIFEST).unwrap();
        }
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let provider = RaSemanticProvider::workspace_scope(root.clone());
        Project {
            _dir: dir,
            root,
            provider,
        }
    }

    pub fn path(&self, file: &str) -> PathBuf {
        self.root.join(file)
    }

    fn text(&self, file: &str) -> String {
        std::fs::read_to_string(self.path(file)).unwrap()
    }

    /// The range of `needle` in `file`; `needle#2` is its second occurrence.
    pub fn range(&self, file: &str, needle: &str) -> ByteRange {
        let (needle, nth) = match needle.rsplit_once('#') {
            Some((needle, n)) if n.parse::<usize>().is_ok() => (needle, n.parse().unwrap()),
            _ => (needle, 1),
        };
        let text = self.text(file);
        let start = text
            .match_indices(needle)
            .nth(nth - 1)
            .unwrap_or_else(|| panic!("{needle:?} #{nth} not in {file}"))
            .0;
        ByteRange::new(start as u32, (start + needle.len()) as u32)
    }

    /// `file:text`, for a range of a file the answer names.
    fn describe(&self, file: &std::path::Path, content: &str, range: ByteRange) -> String {
        let relative = file
            .strip_prefix(&self.root)
            .unwrap_or(file)
            .to_string_lossy();
        let text = &content[range.start as usize..range.end as usize];
        format!("{relative}:{}", first_line(text))
    }

    /// Where the name at `needle` is declared, as `file:text`.
    pub fn definition(&self, file: &str, needle: &str) -> Option<String> {
        let found = self
            .provider
            .get_definition(
                &self.path(file),
                self.range(file, needle),
                Default::default(),
            )
            .unwrap()?;
        Some(self.describe(
            &found.location.file_path,
            &found.content,
            found.location.range,
        ))
    }

    /// Every use of the name at `needle`, as `file:text`.
    pub fn references(&self, file: &str, needle: &str) -> Vec<String> {
        let found = self
            .provider
            .find_references(&self.path(file), self.range(file, needle))
            .unwrap();
        found
            .files
            .iter()
            .flat_map(|f| {
                f.locations
                    .iter()
                    .map(|l| self.describe(&f.file_path, &f.content, l.range))
            })
            .collect()
    }

    pub fn type_of(&self, file: &str, needle: &str) -> Option<String> {
        self.provider
            .get_type(&self.path(file), self.range(file, needle))
            .unwrap()
    }

    fn calls(&self, file: &str, needle: &str, direction: CallGraphDirection) -> Vec<String> {
        let options = CallGraphOptions {
            direction,
            ..Default::default()
        };
        let found = self
            .provider
            .get_call_graph(&self.path(file), self.range(file, needle), options)
            .unwrap();
        let mut out: Vec<String> = found
            .files
            .iter()
            .flat_map(|f| {
                f.entries.iter().map(|e| {
                    // A function as its header: `pub fn vat(net: u32) -> u32`.
                    let name = self.describe(&f.file_path, &f.content, e.range);
                    let name = name.split(" {").next().unwrap_or(&name).to_string();
                    if e.resolved {
                        format!("{name} ({})", e.via.as_str())
                    } else {
                        format!("{name} (unresolved)")
                    }
                })
            })
            .collect();
        out.sort();
        out
    }

    /// The functions the function at `needle` calls: `file:fn header (via)`.
    pub fn callees(&self, file: &str, needle: &str) -> Vec<String> {
        self.calls(file, needle, CallGraphDirection::Callees)
    }

    /// The functions that call the function at `needle`.
    pub fn callers(&self, file: &str, needle: &str) -> Vec<String> {
        self.calls(file, needle, CallGraphDirection::Callers)
    }

    fn flow(&self, file: &str, needle: &str, direction: FlowDirection) -> Vec<String> {
        let options = FlowOptions {
            direction,
            interprocedural: true,
            ..Default::default()
        };
        let found = self
            .provider
            .get_data_flow(&self.path(file), self.range(file, needle), options)
            .unwrap();
        let mut out: Vec<String> = found
            .files
            .iter()
            .flat_map(|f| {
                f.steps
                    .iter()
                    .map(|s| self.describe(&f.file_path, &f.content, s.range))
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Where the value at `needle` comes from.
    pub fn sources(&self, file: &str, needle: &str) -> Vec<String> {
        self.flow(file, needle, FlowDirection::Backward)
    }

    /// Where the value at `needle` goes.
    pub fn uses(&self, file: &str, needle: &str) -> Vec<String> {
        self.flow(file, needle, FlowDirection::Forward)
    }

    /// Every entry point in the workspace, as `kind label`.
    pub fn entry_points(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .provider
            .list_entry_points("rust")
            .unwrap()
            .into_iter()
            .flat_map(|f| f.entry_points)
            .map(|e| format!("{} {}", e.kind, e.label))
            .collect();
        out.sort();
        out
    }
}

fn first_line(text: &str) -> &str {
    let line = text.lines().next().unwrap_or("");
    line.trim_end_matches('{').trim_end()
}

/// Whether `list` has an item containing `wanted`.
pub fn has(list: &[String], wanted: &str) -> bool {
    list.iter().any(|item| item.contains(wanted))
}
