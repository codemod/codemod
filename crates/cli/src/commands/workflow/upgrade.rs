//! `codemod workflow upgrade`: convert workflow.yaml files to workflow.ts.
//!
//! PATH may be a package (a directory with codemod.yaml or workflow.yaml), a
//! workflow file, or a directory of packages such as a monorepo: then every
//! package below it is upgraded.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use clap::Args;
use codemod_workflow::compat::translate_file;
use console::style;

use crate::utils::manifest::CodemodManifest;

#[derive(Args, Debug)]
pub struct Command {
    /// Package directory or workflow file to upgrade (default: current directory)
    #[arg(value_name = "PATH")]
    path: Option<PathBuf>,

    /// Keep the YAML files next to the new workflow.ts files
    #[arg(long)]
    keep_yaml: bool,

    /// Print the generated TypeScript instead of writing files
    #[arg(long)]
    dry_run: bool,
}

/// One workflow file to convert.
struct Conversion {
    yaml: PathBuf,
    typescript: PathBuf,
    /// The path as written in codemod.yaml, relative to the package.
    manifest_path: Option<String>,
}

pub fn handler(args: &Command) -> Result<()> {
    let path = match &args.path {
        Some(path) => path.clone(),
        None => std::env::current_dir()?,
    };
    let (package_dir, conversions) = plan(&path)?;
    if !conversions.is_empty() {
        upgrade_package(&package_dir, &conversions, args)?;
        if !args.dry_run {
            println!(
                "\nCheck the result with: codemod workflow validate -w {}",
                package_dir.display()
            );
            types_hint(&package_dir);
        }
        return Ok(());
    }

    // Not a package with YAML to convert: look for packages below it.
    let packages = if path.is_dir() && !is_package(&path) {
        find_packages(&path)?
    } else {
        Vec::new()
    };
    let mut upgraded = 0;
    for package in &packages {
        let (package_dir, conversions) = plan(package)?;
        if conversions.is_empty() {
            continue;
        }
        println!("{}", style(relative(&path, &package_dir)).bold());
        upgrade_package(&package_dir, &conversions, args)?;
        upgraded += 1;
    }
    if upgraded == 0 {
        println!(
            "Nothing to upgrade: no workflow.yaml found in {}",
            path.display()
        );
    } else if !args.dry_run {
        println!(
            "\nUpgraded {upgraded} package{}. Check each with: codemod workflow validate -w <package>",
            if upgraded == 1 { "" } else { "s" }
        );
        types_hint(&path);
    }
    Ok(())
}

/// The generated files reference `codemod/workflow`, which ships with the
/// `codemod` package; suggest depending on it when the nearest package.json
/// does not.
fn types_hint(dir: &Path) {
    let manifest = dir
        .canonicalize()
        .unwrap_or_else(|_| dir.to_path_buf())
        .ancestors()
        .map(|ancestor| ancestor.join("package.json"))
        .find(|candidate| candidate.is_file());
    let Some(manifest) = manifest else {
        return;
    };
    let has_codemod = std::fs::read_to_string(&manifest)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .is_some_and(|json| {
            ["dependencies", "devDependencies"]
                .iter()
                .any(|field| json[field].get("codemod").is_some())
        });
    if !has_codemod {
        println!(
            "{} Add codemod to {}, so its scripts run that CLI and editors get the workflow types: npm install codemod",
            style("i").cyan(),
            manifest.display()
        );
    }
}

fn is_package(dir: &Path) -> bool {
    dir.join("codemod.yaml").is_file()
        || butterflow_core::package_workflows::DEFAULT_WORKFLOW_FILES
            .iter()
            .any(|name| dir.join(name).is_file())
}

/// Directories below `root` with a codemod.yaml, skipping dependencies,
/// build output and hidden directories.
fn find_packages(root: &Path) -> Result<Vec<PathBuf>> {
    let mut packages = Vec::new();
    let walker = walkdir::WalkDir::new(root)
        .min_depth(1)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !(entry.file_type().is_dir()
                && (name.starts_with('.')
                    || matches!(name.as_ref(), "node_modules" | "target" | "dist")))
        });
    for entry in walker {
        let entry = entry?;
        if entry.file_type().is_file() && entry.file_name() == "codemod.yaml" {
            if let Some(parent) = entry.path().parent() {
                packages.push(parent.to_path_buf());
            }
        }
    }
    Ok(packages)
}

/// Convert one package's workflow files and point its codemod.yaml at them.
fn upgrade_package(package_dir: &Path, conversions: &[Conversion], args: &Command) -> Result<()> {
    // Translate everything first: a workflow that cannot be converted must
    // not leave the package half upgraded.
    let mut translated = Vec::new();
    for conversion in conversions {
        translated.push(translate_file(&conversion.yaml).map_err(|e| anyhow!(e))?);
    }
    if args.dry_run {
        for (conversion, translation) in conversions.iter().zip(&translated) {
            println!(
                "{}\n{}",
                style(format!("// {}", conversion.typescript.display())).dim(),
                translation.source
            );
        }
        return Ok(());
    }
    if let Some(taken) = conversions.iter().find(|c| c.typescript.exists()) {
        return Err(anyhow!(
            "{} already exists; remove it or move it before upgrading",
            taken.typescript.display()
        ));
    }

    let manifest_path = package_dir.join("codemod.yaml");
    let manifest = read_manifest(package_dir)?;
    let mut updated_manifest = manifest.clone();
    for (conversion, translation) in conversions.iter().zip(&translated) {
        std::fs::write(&conversion.typescript, &translation.source)
            .with_context(|| format!("cannot write {}", conversion.typescript.display()))?;
        if let (Some(text), Some(old)) = (updated_manifest.as_mut(), &conversion.manifest_path) {
            *text = point_manifest_at(text, old, &file_name(&conversion.typescript));
        }
    }
    // The manifest points at the new files before the old ones go.
    let manifest_changed = updated_manifest != manifest;
    if let Some(text) = updated_manifest.filter(|_| manifest_changed) {
        std::fs::write(&manifest_path, text).context("cannot update codemod.yaml")?;
    }
    for (conversion, translation) in conversions.iter().zip(&translated) {
        if !args.keep_yaml {
            std::fs::remove_file(&conversion.yaml)
                .with_context(|| format!("cannot remove {}", conversion.yaml.display()))?;
        }
        println!(
            "{} {} -> {}",
            style("✓").green(),
            relative(package_dir, &conversion.yaml),
            relative(package_dir, &conversion.typescript)
        );
        for warning in &translation.warnings {
            println!("  {} {warning}", style("!").yellow());
        }
    }
    if manifest_changed {
        println!("{} updated codemod.yaml", style("✓").green());
    }
    Ok(())
}

/// The package directory and the workflow files to convert.
fn plan(path: &Path) -> Result<(PathBuf, Vec<Conversion>)> {
    if path.is_file() {
        // One workflow of a package: its codemod.yaml entry, if it has one,
        // moves with it.
        let package_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let entry = manifest_entries(&package_dir)
            .ok()
            .flatten()
            .unwrap_or_default()
            .into_iter()
            .find(|entry| same_file(&package_dir.join(entry), path));
        return Ok((package_dir, vec![conversion(path, entry)]));
    }
    if !path.is_dir() {
        return Err(anyhow!("{} does not exist", path.display()));
    }
    if let Some(entries) = manifest_entries(path)? {
        let conversions = entries
            .into_iter()
            .map(|entry| (path.join(&entry), entry))
            .filter(|(file, _)| is_yaml(file) && file.is_file())
            .map(|(file, entry)| conversion(&file, Some(entry)))
            .collect();
        return Ok((path.to_path_buf(), conversions));
    }
    // No codemod.yaml: the workflow a run would pick, if it is YAML.
    let conversions = butterflow_core::package_workflows::locate_workflow(path, None)
        .ok()
        .filter(|file| is_yaml(file))
        .map(|file| conversion(&file, None))
        .into_iter()
        .collect();
    Ok((path.to_path_buf(), conversions))
}

/// The workflow paths a package's codemod.yaml lists, as written there;
/// `None` without a codemod.yaml.
fn manifest_entries(package_dir: &Path) -> Result<Option<Vec<String>>> {
    let manifest_path = package_dir.join("codemod.yaml");
    if !manifest_path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&manifest_path)?;
    let manifest: CodemodManifest = serde_yaml::from_str(&text)
        .with_context(|| format!("cannot parse {}", manifest_path.display()))?;
    Ok(Some(
        manifest
            .resolved_workflows()?
            .into_iter()
            .map(|entry| entry.path)
            .collect(),
    ))
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn conversion(yaml: &Path, manifest_path: Option<String>) -> Conversion {
    Conversion {
        typescript: yaml.with_extension("ts"),
        yaml: yaml.to_path_buf(),
        manifest_path,
    }
}

fn is_yaml(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("yaml" | "yml" | "json")
    )
}

fn read_manifest(package_dir: &Path) -> Result<Option<String>> {
    let path = package_dir.join("codemod.yaml");
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(std::fs::read_to_string(path)?))
}

/// Point codemod.yaml at the new file, keeping the rest of the file as is.
/// A manifest without a workflow entry gets `workflow: <new>`.
fn point_manifest_at(text: &str, old: &str, new: &str) -> String {
    let plain = |path: &str| path.trim().trim_start_matches("./").to_string();
    let old_name = plain(old);
    let new_path = match Path::new(&old_name).parent() {
        Some(parent) if !parent.as_os_str().is_empty() => format!("{}/{new}", parent.display()),
        _ => new.to_string(),
    };
    let mut replaced = false;
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            // `workflow: <path>` or, in a `workflows:` list, `path: <path>`.
            let key = line.trim_start().trim_start_matches("- ");
            let Some(value) = key
                .strip_prefix("workflow:")
                .or_else(|| key.strip_prefix("path:"))
            else {
                return line.to_string();
            };
            // The value up to a comment, without quotes: only this exact
            // path is the entry, not one that merely contains it.
            let written = value.split(" #").next().unwrap_or(value).trim();
            let unquoted = written.trim_matches(|c| c == '"' || c == '\'');
            if written.is_empty() || plain(unquoted) != old_name {
                return line.to_string();
            }
            replaced = true;
            let quote = written
                .chars()
                .next()
                .filter(|c| *c == '"' || *c == '\'')
                .map(String::from)
                .unwrap_or_default();
            let at = line.len() - value.len();
            format!(
                "{}{}",
                &line[..at],
                value.replacen(written, &format!("{quote}{new_path}{quote}"), 1)
            )
        })
        .collect();
    let mut result = lines.join("\n");
    if text.ends_with('\n') {
        result.push('\n');
    }
    if !replaced
        && !text
            .lines()
            .any(|l| l.starts_with("workflow:") || l.starts_with("workflows:"))
    {
        if !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str(&format!("workflow: {new}\n"));
    }
    result
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn relative(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_paths_are_rewritten_in_place() {
        let text =
            "name: demo\nworkflows:\n  - name: main\n    default: true\n    path: workflow.yaml\n";
        assert_eq!(
            point_manifest_at(text, "workflow.yaml", "workflow.ts"),
            "name: demo\nworkflows:\n  - name: main\n    default: true\n    path: workflow.ts\n"
        );
        assert_eq!(
            point_manifest_at("workflow: \"flows/a.yaml\"\n", "flows/a.yaml", "a.ts"),
            "workflow: \"flows/a.ts\"\n"
        );
    }

    #[test]
    fn a_manifest_without_a_workflow_entry_gains_one() {
        assert_eq!(
            point_manifest_at("name: demo\n", "workflow.yaml", "workflow.ts"),
            "name: demo\nworkflow: workflow.ts\n"
        );
    }

    #[test]
    fn upgrading_a_package_writes_workflow_ts_and_updates_the_manifest() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("codemod.yaml"),
            "schema_version: \"1.0\"\nname: demo\nversion: 0.1.0\ndescription: d\nauthor: a\nworkflow: workflow.yaml\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("workflow.yaml"),
            "version: \"1\"\nnodes:\n  - id: a\n    name: A\n    steps:\n      - name: Hello\n        run: echo hi\n",
        )
        .unwrap();
        handler(&Command {
            path: Some(dir.path().to_path_buf()),
            keep_yaml: false,
            dry_run: false,
        })
        .unwrap();
        let source = std::fs::read_to_string(dir.path().join("workflow.ts")).unwrap();
        assert!(
            source.contains("await step(\"Hello\", shell(\"echo hi\"));"),
            "{source}"
        );
        assert!(!dir.path().join("workflow.yaml").exists());
        let manifest = std::fs::read_to_string(dir.path().join("codemod.yaml")).unwrap();
        assert!(manifest.contains("workflow: workflow.ts"), "{manifest}");
        assert!(codemod_workflow::load_workflow(&dir.path().join("workflow.ts")).is_ok());
    }

    #[test]
    fn upgrading_a_directory_of_packages_upgrades_each_one() {
        let dir = tempfile::TempDir::new().unwrap();
        for name in ["recipes/a", "recipes/b", "node_modules/dep"] {
            let package = dir.path().join(name);
            std::fs::create_dir_all(&package).unwrap();
            std::fs::write(
                package.join("codemod.yaml"),
                "schema_version: \"1.0\"\nname: demo\nversion: 0.1.0\ndescription: d\nauthor: a\nworkflow: workflow.yaml\n",
            )
            .unwrap();
            std::fs::write(
                package.join("workflow.yaml"),
                "version: \"1\"\nnodes:\n  - id: a\n    name: A\n    steps:\n      - name: Hello\n        run: echo hi\n",
            )
            .unwrap();
        }
        handler(&Command {
            path: Some(dir.path().to_path_buf()),
            keep_yaml: false,
            dry_run: false,
        })
        .unwrap();
        for name in ["recipes/a", "recipes/b"] {
            assert!(
                dir.path().join(name).join("workflow.ts").is_file(),
                "{name}"
            );
            assert!(
                !dir.path().join(name).join("workflow.yaml").exists(),
                "{name}"
            );
        }
        assert!(dir.path().join("node_modules/dep/workflow.yaml").exists());
    }

    #[test]
    fn the_manifest_entry_is_matched_exactly() {
        let manifest = "workflows:\n  - name: main\n    path: \"./workflow.yaml\" # the default\n  - name: other\n    path: my-workflow.yaml\n";
        let updated = point_manifest_at(manifest, "./workflow.yaml", "workflow.ts");
        assert_eq!(
            updated,
            "workflows:\n  - name: main\n    path: \"workflow.ts\" # the default\n  - name: other\n    path: my-workflow.yaml\n"
        );
    }

    #[test]
    fn upgrading_one_file_moves_its_manifest_entry() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("codemod.yaml"),
            "schema_version: \"1.0\"\nname: demo\nversion: 0.1.0\ndescription: d\nauthor: a\nlicense: MIT\nworkflow: workflow.yaml\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("workflow.yaml"),
            "version: \"1\"\nnodes:\n  - id: a\n    name: A\n    steps:\n      - name: Hi\n        run: echo hi\n",
        )
        .unwrap();
        let args = Command {
            path: Some(dir.path().join("workflow.yaml")),
            keep_yaml: false,
            dry_run: false,
        };
        handler(&args).unwrap();
        assert!(dir.path().join("workflow.ts").is_file());
        assert!(!dir.path().join("workflow.yaml").exists());
        let manifest = std::fs::read_to_string(dir.path().join("codemod.yaml")).unwrap();
        assert!(manifest.contains("workflow: workflow.ts"), "{manifest}");
    }

    #[test]
    fn a_workflow_that_cannot_be_converted_leaves_the_package_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = "schema_version: \"1.0\"\nname: demo\nversion: 0.1.0\ndescription: d\nauthor: a\nlicense: MIT\nworkflows:\n  - name: good\n    default: true\n    path: good.yaml\n  - name: bad\n    path: bad.yaml\n";
        std::fs::write(dir.path().join("codemod.yaml"), manifest).unwrap();
        std::fs::write(
            dir.path().join("good.yaml"),
            "version: \"1\"\nnodes:\n  - id: a\n    name: A\n    steps:\n      - name: Hi\n        run: echo hi\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("bad.yaml"), "version: \"1\"\nnodes: [").unwrap();
        let args = Command {
            path: Some(dir.path().to_path_buf()),
            keep_yaml: false,
            dry_run: false,
        };
        assert!(handler(&args).is_err());
        assert!(dir.path().join("good.yaml").is_file());
        assert!(!dir.path().join("good.ts").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("codemod.yaml")).unwrap(),
            manifest
        );
    }
}
