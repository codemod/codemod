//! The engine side of two contracts TypeScript relies on:
//!
//! - `packages/orchestration/fixtures/walker/cases.json`: the workflow
//!   engine's walker (`codemod_walk_builder` plus `OverrideBuilder`
//!   include/exclude globs, as `CodemodExecutionConfig::build_globs` wires
//!   them) must select exactly `expected`; `tests/files.test.ts` runs the
//!   TypeScript walker against the same cases.
//! - `packages/orchestration/src/languages.json`: the language-extension
//!   table TypeScript uses for definitions without `include` must equal the
//!   engine's `create_language_extension_map`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use codemod_sandbox::sandbox::{
    engine::{codemod_lang::CodemodLang, language_data::create_language_extension_map},
    filesystem::codemod_walk_builder,
};
use ignore::overrides::OverrideBuilder;
use serde::Deserialize;

const ORCHESTRATION: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packages/orchestration");

#[derive(Deserialize)]
struct Contract {
    files: BTreeMap<String, String>,
    #[serde(default)]
    symlinks: BTreeMap<String, String>,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    #[serde(default)]
    include: Option<Vec<String>>,
    #[serde(default)]
    exclude: Option<Vec<String>>,
    expected: Vec<String>,
}

fn walk(root: &Path, include: Option<&[String]>, exclude: Option<&[String]>) -> Vec<String> {
    let mut builder = codemod_walk_builder(root);
    if include.is_some() || exclude.is_some() {
        let mut overrides = OverrideBuilder::new(root);
        for glob in include.unwrap_or_default() {
            overrides.add(glob).expect("include glob");
        }
        for glob in exclude.unwrap_or_default() {
            overrides.add(&format!("!{glob}")).expect("exclude glob");
        }
        builder.overrides(overrides.build().expect("overrides"));
    }
    let mut files: Vec<PathBuf> = builder
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(|entry| entry.path().strip_prefix(root).unwrap().to_path_buf())
        .collect();
    files.sort();
    files
        .iter()
        .map(|path| {
            path.components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect()
}

#[test]
fn engine_walker_matches_the_shared_contract() {
    let text = std::fs::read_to_string(format!("{ORCHESTRATION}/fixtures/walker/cases.json"))
        .expect("cases.json");
    let contract: Contract = serde_json::from_str(&text).expect("cases.json parses");
    let dir = tempfile::tempdir().expect("tempdir");
    for (relative, content) in &contract.files {
        let path = dir.path().join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, content).expect("write");
    }
    #[cfg(unix)]
    for (link, target) in &contract.symlinks {
        std::os::unix::fs::symlink(dir.path().join(target), dir.path().join(link))
            .expect("symlink");
    }
    for case in &contract.cases {
        let actual = walk(dir.path(), case.include.as_deref(), case.exclude.as_deref());
        assert_eq!(actual, case.expected, "case: {}", case.name);
    }
}

#[test]
fn language_extensions_match_the_engine_table() {
    let text = std::fs::read_to_string(format!("{ORCHESTRATION}/src/languages.json"))
        .expect("languages.json");
    let fixture: BTreeMap<String, Vec<String>> =
        serde_json::from_str(&text).expect("languages.json parses");
    let engine = create_language_extension_map();
    // Keys are the names authors write (`language: "typescript"`), which the
    // engine parses; every engine entry must appear exactly once.
    assert_eq!(
        fixture.len(),
        engine.len(),
        "languages.json must list every engine language"
    );
    for (name, extensions) in &fixture {
        let language: CodemodLang = name
            .parse()
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            engine.get(&language),
            Some(&extensions.iter().map(String::as_str).collect()),
            "{name}"
        );
    }
}
