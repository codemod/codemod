//! The TypeScript walker (`packages/orchestration/src/walker.ts`) must select
//! exactly the files the workflow engine's walker selects. This test runs the
//! engine side of the shared contract in
//! `packages/orchestration/fixtures/walker/cases.json`: `codemod_walk_builder`
//! plus `OverrideBuilder` include/exclude globs, exactly as
//! `CodemodExecutionConfig::build_globs` wires them. The TypeScript test runs
//! the other side against the same expectations.

use std::path::{Path, PathBuf};

use codemod_sandbox::sandbox::filesystem::codemod_walk_builder;
use ignore::overrides::OverrideBuilder;
use serde::Deserialize;

const CASES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/orchestration/fixtures/walker/cases.json"
);

#[derive(Deserialize)]
struct Contract {
    files: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    symlinks: std::collections::BTreeMap<String, String>,
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

fn materialize(contract: &Contract) -> tempfile::TempDir {
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
    dir
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
        .map(|entry| {
            entry
                .path()
                .strip_prefix(root)
                .expect("inside root")
                .to_path_buf()
        })
        .collect();
    // Component-wise, the order the old bridge and the TypeScript walker use.
    files.sort();
    files
        .into_iter()
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
    let text = std::fs::read_to_string(CASES).expect("cases.json");
    let contract: Contract = serde_json::from_str(&text).expect("cases.json parses");
    let dir = materialize(&contract);
    for case in &contract.cases {
        let actual = walk(dir.path(), case.include.as_deref(), case.exclude.as_deref());
        assert_eq!(actual, case.expected, "case: {}", case.name);
    }
}
