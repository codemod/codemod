//! Bundler's `Gemfile.lock`. Gems from rubygems (`GEM`) and git (`GIT`)
//! are nodes, gems from a local path (`PATH`) are workspaces in that path,
//! and the bundle itself is the `<.>` workspace, whose dependencies are the
//! lockfile's `DEPENDENCIES`.
//!
//! A gem built for several platforms (`nokogiri (1.16.0-x86_64-linux)`) is
//! one node at its version. The lockfile keeps runtime dependencies only,
//! with their requirements; which of the bundle's own dependencies are for
//! development comes from the `Gemfile` beside it.

use std::collections::{HashMap, HashSet};

use super::{ReadFile, UNVERSIONED};
use crate::model::{join, Ecosystem, Graph, Kind};

/// What a dependency without a requirement accepts, as Bundler writes it.
const ANY: &str = ">= 0";

/// Gemfile groups whose gems are `dev`; gems in any other group are `prod`.
const DEV_GROUPS: [&str; 2] = ["development", "test"];

/// One `name (requirements)` line: its name and what is in the parentheses.
struct Entry<'a> {
    name: &'a str,
    detail: Option<&'a str>,
}

pub fn parse(text: &str, read: ReadFile) -> Result<Graph, String> {
    let mut graph = Graph::from_lockfile("Gemfile.lock");
    let mut by_name = HashMap::<String, usize>::new();
    // Each spec's node and its dependency lines.
    let mut specs: Vec<(usize, Vec<Entry>)> = Vec::new();
    let mut declared = Vec::new();
    let mut section = "";
    let mut remote = ".";
    let mut known = false;

    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 0 {
            section = line.trim();
            remote = ".";
            known |= matches!(section, "GEM" | "GIT" | "PATH" | "DEPENDENCIES");
            continue;
        }
        if let Some(value) = line.trim().strip_prefix("remote:") {
            remote = value.trim();
            continue;
        }
        let Some(entry) = entry(line.trim()) else {
            continue;
        };
        match (section, indent) {
            ("GEM" | "GIT" | "PATH", 4) => {
                // `1.16.0-x86_64-linux` is 1.16.0 built for one platform.
                let version = entry.detail.unwrap_or_default();
                let version = version.split('-').next().unwrap_or_default();
                let id = if section == "PATH" {
                    graph.workspace(Ecosystem::Rubygems, entry.name, version, &join(".", remote))
                } else {
                    graph.node(Ecosystem::Rubygems, entry.name, version)
                };
                by_name.entry(entry.name.to_string()).or_insert(id);
                specs.push((id, Vec::new()));
            }
            ("GEM" | "GIT" | "PATH", 6) => {
                if let Some((_, dependencies)) = specs.last_mut() {
                    dependencies.push(entry);
                }
            }
            ("DEPENDENCIES", 2) => declared.push((entry, line.trim_end().ends_with('!'))),
            _ => {}
        }
    }
    if !known {
        return Err("not a Gemfile.lock: no GEM, GIT, PATH or DEPENDENCIES section".into());
    }

    for (from, dependencies) in specs {
        for dependency in dependencies {
            let to = by_name.get(dependency.name).copied();
            let range = dependency.detail.unwrap_or(ANY).to_string();
            graph.edge(from, to, dependency.name, Some(range), Kind::Prod);
        }
    }

    let gemfile = read("Gemfile").map(|text| Gemfile::parse(&text));
    let root = graph.workspace(Ecosystem::Rubygems, "<.>", UNVERSIONED, ".");
    for (dependency, from_source) in declared {
        let kind = match &gemfile {
            Some(gemfile) => gemfile.kind(dependency.name, from_source),
            None => Kind::Prod,
        };
        let to = by_name.get(dependency.name).copied();
        let range = dependency.detail.unwrap_or(ANY).to_string();
        graph.edge(root, to, dependency.name, Some(range), kind);
    }
    Ok(graph)
}

/// `rack (>= 2.2, < 4)!` as its name and `>= 2.2, < 4`.
fn entry(line: &str) -> Option<Entry<'_>> {
    let line = line.trim_end_matches('!');
    let (name, detail) = match line.split_once(" (") {
        Some((name, rest)) => (name, Some(rest.strip_suffix(')')?)),
        None => (line, None),
    };
    (!name.is_empty() && !name.contains([' ', ':'])).then_some(Entry { name, detail })
}

/// What a Gemfile says about each gem's groups, read line by line rather
/// than run: gems named through variables or loops are not seen.
struct Gemfile {
    /// Gems the Gemfile names, with whether every declaration of each is in
    /// development or test groups only.
    dev_only: HashMap<String, bool>,
    /// A `gemspec` line adds the gemspec's development dependencies to
    /// DEPENDENCIES without naming them here.
    gemspec: bool,
}

impl Gemfile {
    fn parse(text: &str) -> Self {
        let mut dev_only = HashMap::<String, bool>::new();
        let mut gemspec = false;
        // Groups of each open `do` / `if` block, innermost last.
        let mut blocks: Vec<Vec<String>> = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let word = line
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .next()
                .unwrap_or_default();
            match word {
                "end" => {
                    blocks.pop();
                }
                "gemspec" => gemspec = true,
                "gem" => {
                    let Some(name) = quoted(line) else { continue };
                    let mut groups: HashSet<String> = blocks.iter().flatten().cloned().collect();
                    for key in ["group:", "groups:", ":group =>", ":groups =>"] {
                        if let Some((_, rest)) = line.split_once(key) {
                            let rest = rest.trim_start();
                            let value = match rest.strip_prefix('[') {
                                Some(list) => list.split(']').next(),
                                None => rest.split(',').next(),
                            };
                            groups.extend(symbols(value.unwrap_or_default()));
                        }
                    }
                    let dev = !groups.is_empty()
                        && groups
                            .iter()
                            .all(|group| DEV_GROUPS.contains(&group.as_str()));
                    *dev_only.entry(name.to_string()).or_insert(dev) &= dev;
                }
                "group" if opens_block(line) => {
                    let names = line[word.len()..].rsplit_once("do").map_or("", |(n, _)| n);
                    // `optional: true` and other options name no group.
                    let names = names.split(',').filter(|part| !part.contains(": "));
                    blocks.push(names.flat_map(symbols).collect());
                }
                "if" | "unless" | "case" | "begin" | "while" | "until" => blocks.push(Vec::new()),
                _ if opens_block(line) => blocks.push(Vec::new()),
                _ => {}
            }
        }
        Self { dev_only, gemspec }
    }

    fn kind(&self, name: &str, from_source: bool) -> Kind {
        let dev = match self.dev_only.get(name) {
            Some(&dev) => dev,
            None => self.gemspec && !from_source,
        };
        if dev {
            Kind::Dev
        } else {
            Kind::Prod
        }
    }
}

fn opens_block(line: &str) -> bool {
    let line = line.split(" #").next().unwrap_or(line).trim_end();
    let line = match line.strip_suffix('|') {
        Some(rest) => rest.rsplit_once('|').map_or(rest, |(before, _)| before),
        None => line,
    };
    line.trim_end().ends_with(" do")
}

/// The first quoted string on the line.
fn quoted(line: &str) -> Option<&str> {
    let start = line.find(['"', '\''])?;
    let quote = &line[start..=start];
    let rest = &line[start + 1..];
    rest.split_once(quote).map(|(name, _)| name)
}

/// `:development, :test` or `"test"` as group names.
fn symbols(text: &str) -> Vec<String> {
    text.split([',', '[', '(', ')', ' '])
        .map(|part| {
            part.trim()
                .trim_start_matches(':')
                .trim_matches(['"', '\''])
        })
        .filter(|part| {
            !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
        .map(String::from)
        .collect()
}
