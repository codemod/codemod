//! `symbols("python").references` in workspace scope: every Python file under
//! the workspace root is searched, whichever files the run told the provider
//! about, and a text the run tells it about later is what later answers see.

use language_core::{ByteRange, SemanticProvider};
use language_python::RuffSemanticProvider;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const UTILS: &str = "def calculate(x, y):\n    return x * y\n";
const MAIN: &str = "from utils import calculate\n\nprint(calculate(3, 4))\n";

fn workspace(files: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for (path, text) in files {
        std::fs::write(root.join(path), text).unwrap();
    }
    (dir, root)
}

/// The references of `calculate` at its declaration in utils.py, as
/// `file:line`, sorted.
fn references_of_calculate(provider: &RuffSemanticProvider, root: &Path) -> Vec<String> {
    let start = UTILS.find("calculate").unwrap() as u32;
    let found = provider
        .find_references(
            &root.join("utils.py"),
            ByteRange::new(start, start + "calculate".len() as u32),
        )
        .unwrap();
    let mut refs: Vec<String> = found
        .files
        .iter()
        .flat_map(|file| {
            file.locations.iter().map(|at| {
                let line = file.content[..at.range.start as usize]
                    .matches('\n')
                    .count()
                    + 1;
                format!(
                    "{}:{line}",
                    at.file_path.strip_prefix(root).unwrap().display()
                )
            })
        })
        .collect();
    refs.sort();
    refs
}

#[test]
fn references_reach_a_module_the_run_did_not_tell_the_provider_about() {
    let (_dir, root) = workspace(&[("utils.py", UTILS), ("main.py", MAIN)]);
    let provider = RuffSemanticProvider::workspace_scope(root.clone());
    provider
        .notify_file_processed(&root.join("utils.py"), UTILS)
        .unwrap();
    assert_eq!(
        references_of_calculate(&provider, &root),
        ["main.py:1", "main.py:3", "utils.py:1"]
    );
}

#[test]
fn a_text_the_run_tells_the_provider_about_after_a_query_is_what_the_next_query_sees() {
    let (_dir, root) = workspace(&[("utils.py", UTILS), ("main.py", MAIN)]);
    let provider = RuffSemanticProvider::workspace_scope(root.clone());
    assert_eq!(
        references_of_calculate(&provider, &root),
        ["main.py:1", "main.py:3", "utils.py:1"]
    );
    let edited = format!("{MAIN}calculate(5, 6)\n");
    provider
        .notify_file_processed(&root.join("main.py"), &edited)
        .unwrap();
    assert_eq!(
        references_of_calculate(&provider, &root),
        ["main.py:1", "main.py:3", "main.py:4", "utils.py:1"]
    );
}
