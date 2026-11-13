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
    ["codemod.yaml", "workflow.yaml", "workflow.yml"]
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
    let mut manifest_text = read_manifest(package_dir)?;
    for conversion in conversions {
        let translated = translate_file(&conversion.yaml).map_err(|e| anyhow!(e))?;
        if args.dry_run {
            println!(
                "{}\n{}",
                style(format!("// {}", conversion.typescript.display())).dim(),
                translated.source
            );
            continue;
        }
        if conversion.typescript.exists() {
            return Err(anyhow!(
                "{} already exists; remove it or move it before upgrading",
                conversion.typescript.display()
            ));
        }
        std::fs::write(&conversion.typescript, &translated.source)
            .with_context(|| format!("cannot write {}", conversion.typescript.display()))?;
        if let (Some(text), Some(old)) = (manifest_text.as_mut(), &conversion.manifest_path) {
            *text = point_manifest_at(text, old, &file_name(&conversion.typescript));
        }
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
        for warning in &translated.warnings {
            println!("  {} {warning}", style("!").yellow());
        }
    }

    if let Some(text) = manifest_text.filter(|_| !args.dry_run) {
        std::fs::write(package_dir.join("codemod.yaml"), text)
            .context("cannot update codemod.yaml")?;
        println!("{} updated codemod.yaml", style("✓").green());
    }
    Ok(())
}

/// The package directory and the workflow files to convert.
fn plan(path: &Path) -> Result<(PathBuf, Vec<Conversion>)> {
    if path.is_file() {
        let package_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        return Ok((package_dir, vec![conversion(path, None)]));
    }
    if !path.is_dir() {
        return Err(anyhow!("{} does not exist", path.display()));
    }
    let manifest_path = path.join("codemod.yaml");
    if manifest_path.is_file() {
        let text = std::fs::read_to_string(&manifest_path)?;
        let manifest: CodemodManifest = serde_yaml::from_str(&text)
            .with_context(|| format!("cannot parse {}", manifest_path.display()))?;
        let mut conversions = Vec::new();
        for entry in manifest.resolved_workflows()? {
            let file = path.join(&entry.path);
            if is_yaml(&file) && file.is_file() {
                conversions.push(conversion(&file, Some(entry.path.clone())));
            }
        }
        return Ok((path.to_path_buf(), conversions));
    }
    let conversions = ["workflow.yaml", "workflow.yml"]
        .iter()
        .map(|name| path.join(name))
        .filter(|file| file.is_file())
        .map(|file| conversion(&file, None))
        .collect();
    Ok((path.to_path_buf(), conversions))
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
    let old_name = old.trim().trim_start_matches("./");
    let mut replaced = false;
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            let key = line.trim_start().trim_start_matches("- ");
            let is_workflow_line = key.starts_with("workflow:") || key.starts_with("path:");
            if is_workflow_line && line.contains(old_name) {
                replaced = true;
                let new_path = match Path::new(old_name).parent() {
                    Some(parent) if !parent.as_os_str().is_empty() => {
                        format!("{}/{new}", parent.display())
                    }
                    _ => new.to_string(),
                };
                line.replacen(old_name, &new_path, 1)
            } else {
                line.to_string()
            }
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
}
