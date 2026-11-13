//! `codemod workflow test`: run the whole workflow on each fixture and
//! compare the files it leaves with the fixture's expected output.
//!
//! A fixture is a directory with either `input/` and `expected/`
//! directories, or `input.<ext>` and `expected.<ext>` files (the layouts
//! `codemod jssg test` reads). An optional `params.json` holds the run's
//! parameters.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::Args;
use codemod_workflow::TestPlan;
use console::style;
use serde_json::{Map, Value};
use testing_utils::Strictness;

use crate::engine::create_registry_client;
use crate::utils::resolve_capabilities::{resolve_capabilities, ResolveCapabilitiesArgs};
use crate::utils::workflow_source::resolve_workflow_source_with_name;
use crate::workflow_exec::WorkflowFailureAlreadyReported;
use crate::workflow_test::{copy_into, read_tree, run_case, write_tree, CaseRequest};

#[derive(Args, Debug)]
pub struct Command {
    /// Directory of fixtures, one per subdirectory
    #[arg(default_value = "tests")]
    tests: PathBuf,

    /// Path to a workflow file (workflow.ts or workflow.yaml) or a package directory
    #[arg(short, long, value_name = "PATH", default_value = ".")]
    workflow: String,

    /// Name of the workflow to test when codemod.yaml defines multiple workflows
    #[arg(long = "workflow-name", value_name = "NAME")]
    workflow_name: Option<String>,

    /// Write each run's files into its fixture's expected output
    #[arg(short = 'u', long)]
    update: bool,

    /// Only run fixtures whose name contains this text
    #[arg(long)]
    filter: Option<String>,

    /// Allow fs access
    #[arg(long)]
    allow_fs: bool,

    /// Allow fetch access
    #[arg(long)]
    allow_fetch: bool,

    /// Allow child process access
    #[arg(long)]
    allow_child_process: bool,

    /// Leave manual approvals unanswered (the run then waits, and the test fails)
    #[arg(long)]
    no_approve: bool,

    /// How files are compared: strict (text), cst (syntax trees, with
    /// whitespace), ast (syntax trees), or loose (ast, ignoring the order of
    /// some children). Files in languages without a parser are compared as text.
    #[arg(long, default_value = "strict")]
    strictness: String,
}

#[derive(Debug)]
struct Fixture {
    name: String,
    dir: PathBuf,
    input: PathBuf,
    expected: PathBuf,
    /// `input.<ext>` and `expected.<ext>` rather than directories.
    single_file: bool,
}

pub async fn handler(args: &Command) -> Result<()> {
    let strictness: Strictness = args
        .strictness
        .parse()
        .map_err(|e: String| anyhow::anyhow!(e))?;
    let (workflow, workflow_dir) =
        resolve_workflow_source_with_name(&args.workflow, args.workflow_name.as_deref())?;
    if !args.tests.is_dir() {
        bail!("no fixtures directory at {}", args.tests.display());
    }
    let mut fixtures = discover(&args.tests)?;
    if let Some(filter) = &args.filter {
        fixtures.retain(|fixture| fixture.name.contains(filter.as_str()));
    }
    if fixtures.is_empty() {
        bail!(
            "no fixtures in {}: each needs input/ and expected/, or input.<ext> and expected.<ext>",
            args.tests.display()
        );
    }
    let capabilities = resolve_capabilities(
        ResolveCapabilitiesArgs {
            allow_fs: args.allow_fs,
            allow_fetch: args.allow_fetch,
            allow_child_process: args.allow_child_process,
        },
        None,
        Some(workflow_dir),
    );
    let registry = create_registry_client(None).ok();

    let (mut passed, mut failed, mut updated) = (0, 0, 0);
    for fixture in &fixtures {
        let started = Instant::now();
        let target = tempfile::tempdir().context("cannot create a temporary directory")?;
        copy_into(&fixture.input, target.path())
            .with_context(|| format!("cannot copy {}", fixture.input.display()))?;
        let run = run_case(CaseRequest {
            workflow: workflow.clone(),
            target: target.path().to_path_buf(),
            params: params(&fixture.dir)?,
            plan: TestPlan::default(),
            approve: !args.no_approve,
            capabilities: capabilities.clone(),
            registry: registry.clone(),
        })
        .await;
        let elapsed = style(format!("({:.1}s)", started.elapsed().as_secs_f64())).dim();
        let run = match run {
            Ok(run) => run,
            Err(error) => {
                // The workflow does not build: every fixture would fail the same way.
                return Err(error);
            }
        };
        if let Some(problem) = run.problem() {
            failed += 1;
            println!("{} {} {elapsed}", style("✗").red(), fixture.name);
            println!("    {problem}");
            continue;
        }
        let actual = read_tree(target.path())?;
        let outcome = compare(fixture, &actual, strictness)?;
        match outcome {
            Comparison::Same => {
                passed += 1;
                println!("{} {} {elapsed}", style("✓").green(), fixture.name);
            }
            Comparison::Different(_) | Comparison::NoExpected if args.update => {
                update(fixture, &actual)?;
                updated += 1;
                println!(
                    "{} {} {}",
                    style("↻").yellow(),
                    fixture.name,
                    style("(expected output updated)").dim()
                );
            }
            Comparison::NoExpected => {
                failed += 1;
                println!("{} {} {elapsed}", style("✗").red(), fixture.name);
                println!(
                    "    no expected output at {}; run with -u to create it",
                    fixture.expected.display()
                );
            }
            Comparison::Different(report) => {
                failed += 1;
                println!("{} {} {elapsed}", style("✗").red(), fixture.name);
                for line in report.lines() {
                    println!("    {line}");
                }
            }
        }
    }

    let mut summary = vec![format!("{passed} passed")];
    if failed > 0 {
        summary.push(format!("{failed} failed"));
    }
    if updated > 0 {
        summary.push(format!("{updated} updated"));
    }
    println!("\n{}", summary.join(", "));
    if failed > 0 {
        return Err(WorkflowFailureAlreadyReported.into());
    }
    Ok(())
}

fn params(dir: &Path) -> Result<Map<String, Value>> {
    let path = dir.join("params.json");
    if !path.exists() {
        return Ok(Map::new());
    }
    let text = std::fs::read_to_string(&path)?;
    match serde_json::from_str(&text).with_context(|| format!("invalid {}", path.display()))? {
        Value::Object(map) => Ok(map),
        _ => bail!("{} must hold a JSON object", path.display()),
    }
}

fn discover(root: &Path) -> Result<Vec<Fixture>> {
    let mut fixtures = Vec::new();
    // The fixtures directory may itself hold `input/` and `expected/`.
    if root.join("input").is_dir() {
        let name = std::fs::canonicalize(root)
            .ok()
            .and_then(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| root.display().to_string());
        fixtures.push(Fixture {
            name,
            expected: root.join("expected"),
            input: root.join("input"),
            dir: root.to_path_buf(),
            single_file: false,
        });
    }
    discover_in(root, root, &mut fixtures)?;
    Ok(fixtures)
}

fn discover_in(root: &Path, dir: &Path, fixtures: &mut Vec<Fixture>) -> Result<()> {
    let mut subdirs: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.is_dir())
        // A fixture's own input/ and expected/ are not fixtures.
        .filter(|path| {
            !(dir.join("input").is_dir()
                && path
                    .file_name()
                    .is_some_and(|name| name == "input" || name == "expected"))
        })
        .collect();
    subdirs.sort();
    for sub in subdirs {
        let name = sub
            .strip_prefix(root)
            .unwrap_or(&sub)
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if sub.join("input").is_dir() {
            fixtures.push(Fixture {
                name,
                expected: sub.join("expected"),
                input: sub.join("input"),
                dir: sub,
                single_file: false,
            });
        } else if let Some(input) = input_file(&sub)? {
            let expected = match input.extension() {
                Some(ext) => sub.join("expected").with_extension(ext),
                None => sub.join("expected"),
            };
            fixtures.push(Fixture {
                name,
                expected,
                input,
                dir: sub,
                single_file: true,
            });
        } else {
            discover_in(root, &sub, fixtures)?;
        }
    }
    Ok(())
}

fn input_file(dir: &Path) -> Result<Option<PathBuf>> {
    let mut inputs: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.is_file() && path.file_stem().is_some_and(|stem| stem == "input"))
        .collect();
    inputs.sort();
    match inputs.len() {
        0 => Ok(None),
        1 => Ok(inputs.pop()),
        _ => bail!(
            "{} has more than one input file; use input/ and expected/ directories instead",
            dir.display()
        ),
    }
}

enum Comparison {
    Same,
    NoExpected,
    /// What differs, ready to print.
    Different(String),
}

fn compare(
    fixture: &Fixture,
    actual: &BTreeMap<String, String>,
    strictness: Strictness,
) -> Result<Comparison> {
    if !fixture.expected.exists() {
        return Ok(Comparison::NoExpected);
    }
    if fixture.single_file {
        let expected =
            crate::workflow_test::normalize(&std::fs::read_to_string(&fixture.expected)?);
        return Ok(match sole_file(actual) {
            Ok((path, text)) if same(path, &expected, text, strictness) => Comparison::Same,
            Ok((path, text)) => Comparison::Different(file_diff(path, &expected, text)),
            Err(problem) => Comparison::Different(problem),
        });
    }
    let expected = read_tree(&fixture.expected)?;
    let mut report = Vec::new();
    for (path, text) in &expected {
        match actual.get(path) {
            None => report.push(format!("missing {path}")),
            Some(actual) if !same(path, text, actual, strictness) => {
                report.push(file_diff(path, text, actual))
            }
            Some(_) => {}
        }
    }
    for path in actual.keys().filter(|path| !expected.contains_key(*path)) {
        report.push(format!("unexpected {path}"));
    }
    Ok(if report.is_empty() {
        Comparison::Same
    } else {
        Comparison::Different(report.join("\n"))
    })
}

/// Whether a file matches its expected text at the given strictness.
fn same(path: &str, expected: &str, actual: &str, strictness: Strictness) -> bool {
    if expected == actual || strictness == Strictness::Strict {
        return expected == actual;
    }
    let Some(language) = testing_utils::detect_language(Path::new(path)) else {
        return false;
    };
    let compared = match strictness {
        Strictness::Strict => return false,
        Strictness::Cst => testing_utils::try_cst_compare(expected, actual, language),
        Strictness::Ast => testing_utils::try_ast_compare(expected, actual, language),
        Strictness::Loose => testing_utils::try_loose_compare(expected, actual, language),
    };
    compared.unwrap_or(false)
}

/// The one file a single-file fixture's run leaves.
fn sole_file(tree: &BTreeMap<String, String>) -> Result<(&String, &String), String> {
    let mut files = tree.iter();
    match (files.next(), files.next()) {
        (Some(file), None) => Ok(file),
        (None, _) => Err("the run left no files".into()),
        _ => Err(format!(
            "the run left {} files ({}); use input/ and expected/ directories to test more than one file",
            tree.len(),
            tree.keys().cloned().collect::<Vec<_>>().join(", ")
        )),
    }
}

fn file_diff(path: &str, expected: &str, actual: &str) -> String {
    let diff = similar::TextDiff::from_lines(expected, actual);
    let body = diff
        .unified_diff()
        .context_radius(3)
        .header("expected", "actual")
        .to_string();
    format!("{path} differs:\n{}", body.trim_end())
}

fn update(fixture: &Fixture, actual: &BTreeMap<String, String>) -> Result<()> {
    if fixture.single_file {
        let (_, text) = sole_file(actual).map_err(|problem| anyhow::anyhow!(problem))?;
        std::fs::write(&fixture.expected, text)?;
        return Ok(());
    }
    write_tree(&fixture.expected, actual)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, path: &str, text: &str) {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn discovers_both_fixture_layouts_and_nested_groups() {
        let tests = tempfile::tempdir().unwrap();
        write(tests.path(), "dirs/input/a.ts", "a");
        write(tests.path(), "dirs/expected/a.ts", "a");
        write(tests.path(), "file/input.tsx", "a");
        write(tests.path(), "file/expected.tsx", "a");
        write(tests.path(), "group/inner/input.ts", "a");
        write(tests.path(), "dirs/input/nested/input.ts", "not a fixture");

        let fixtures = discover(tests.path()).unwrap();

        let names: Vec<_> = fixtures.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["dirs", "file", "group/inner"]);
        assert!(!fixtures[0].single_file);
        assert!(fixtures[1].single_file);
        assert!(fixtures[1].expected.ends_with("file/expected.tsx"));
    }

    #[test]
    fn the_fixtures_directory_can_be_a_fixture_itself() {
        let root = tempfile::tempdir().unwrap();
        let tests = root.path().join("tests");
        write(&tests, "input/a.ts", "a");
        write(&tests, "expected/a.ts", "a");
        write(&tests, "other/input.ts", "a");

        let fixtures = discover(&tests).unwrap();

        let names: Vec<_> = fixtures.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["tests", "other"]);
    }

    #[test]
    fn reports_missing_unexpected_and_changed_files() {
        let tests = tempfile::tempdir().unwrap();
        write(tests.path(), "case/input/a.ts", "");
        write(tests.path(), "case/expected/a.ts", "one\n");
        write(tests.path(), "case/expected/b.ts", "b\n");
        let fixture = discover(tests.path()).unwrap().remove(0);
        let actual = BTreeMap::from([
            ("a.ts".to_string(), "two\n".to_string()),
            ("c.ts".to_string(), "c\n".to_string()),
        ]);

        let Comparison::Different(report) = compare(&fixture, &actual, Strictness::Strict).unwrap()
        else {
            panic!("expected a difference");
        };

        assert!(report.contains("a.ts differs:"), "{report}");
        assert!(report.contains("-one\n+two"), "{report}");
        assert!(report.contains("missing b.ts"), "{report}");
        assert!(report.contains("unexpected c.ts"), "{report}");
    }

    #[test]
    fn a_single_file_fixture_compares_the_one_file_the_run_leaves() {
        let tests = tempfile::tempdir().unwrap();
        write(tests.path(), "case/input.ts", "old\r\n");
        write(tests.path(), "case/expected.ts", "new\r\n");
        let fixture = discover(tests.path()).unwrap().remove(0);

        let renamed = BTreeMap::from([("input.mts".to_string(), "new\n".to_string())]);
        assert!(matches!(
            compare(&fixture, &renamed, Strictness::Strict).unwrap(),
            Comparison::Same
        ));

        let two = BTreeMap::from([
            ("input.ts".to_string(), "new\n".to_string()),
            ("extra.ts".to_string(), "".to_string()),
        ]);
        let Comparison::Different(report) = compare(&fixture, &two, Strictness::Strict).unwrap()
        else {
            panic!("expected a difference");
        };
        assert!(report.contains("left 2 files"), "{report}");
    }

    #[test]
    fn cst_strictness_ignores_formatting_only_in_languages_it_parses() {
        let tests = tempfile::tempdir().unwrap();
        write(tests.path(), "case/input/a.ts", "");
        write(tests.path(), "case/expected/a.ts", "const a = 1;\n");
        write(tests.path(), "case/expected/b.txt", "text\n");
        let fixture = discover(tests.path()).unwrap().remove(0);
        let actual = BTreeMap::from([
            ("a.ts".to_string(), "const a = 1;\n\n".to_string()),
            ("b.txt".to_string(), "text\n\n".to_string()),
        ]);

        let Comparison::Different(report) = compare(&fixture, &actual, Strictness::Cst).unwrap()
        else {
            panic!("expected a difference");
        };

        assert!(!report.contains("a.ts"), "{report}");
        assert!(report.contains("b.txt differs"), "{report}");
    }
}
