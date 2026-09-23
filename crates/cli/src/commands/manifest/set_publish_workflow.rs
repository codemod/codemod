use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, ValueEnum};
use regex::{Captures, Regex};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::set_repository::normalize_repository_directory;
use crate::commands::publish::validate_package_name;

const WORKFLOW_TEMPLATE: &str = include_str!("../../templates/common/publish-builder.yml");
const WORKFLOW_DIRECTORY: &str = ".github/workflows";
static WORKFLOW_PLACEHOLDER: OnceLock<Regex> = OnceLock::new();

#[derive(Args, Debug)]
pub struct Command {
    /// Git repository root where .github/workflows is stored
    #[arg(long, value_name = "REPOSITORY_ROOT")]
    repository_root: PathBuf,

    /// Published codemod package name
    #[arg(long)]
    package_name: String,

    /// Package directory relative to the repository root; use `.` for the root
    #[arg(long)]
    directory: String,

    /// Repository default branch that publishes stable versions
    #[arg(long)]
    default_branch: String,

    /// Command output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    output: OutputFormat,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SetPublishWorkflowOutput {
    workflow_path: String,
    repository_directory: Option<String>,
    changed: bool,
}

pub fn handler(args: &Command) -> Result<()> {
    validate_package_name(&args.package_name)?;
    let repository_directory = normalize_repository_directory(&args.directory)?;
    validate_default_branch(&args.default_branch)?;

    let package_hash = package_hash(&args.package_name);
    let workflow_path = workflow_path(&args.package_name, &package_hash);
    let content = render_workflow(
        &args.package_name,
        repository_directory.as_deref(),
        &args.default_branch,
        &package_hash,
    );
    let absolute_workflow_path = args.repository_root.join(&workflow_path);
    let changed = write_managed_workflow(
        &absolute_workflow_path,
        &args.package_name,
        content.as_bytes(),
    )?;
    let output = SetPublishWorkflowOutput {
        workflow_path: path_with_forward_slashes(&workflow_path),
        repository_directory,
        changed,
    };

    match args.output {
        OutputFormat::Text => {
            let action = if changed {
                "Updated"
            } else {
                "Already up to date"
            };
            println!("{action}: {}", absolute_workflow_path.display());
        }
        OutputFormat::Json => println!("{}", serde_json::to_string(&output)?),
    }

    Ok(())
}

fn validate_default_branch(branch: &str) -> Result<()> {
    if branch.trim() != branch || branch.is_empty() || branch.contains(['\r', '\n']) {
        bail!("Default branch is invalid");
    }
    Ok(())
}

fn package_hash(package_name: &str) -> String {
    let digest = Sha256::digest(package_name.as_bytes());
    format!("{digest:x}")[..8].to_string()
}

fn workflow_path(package_name: &str, hash: &str) -> PathBuf {
    let slug = package_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    Path::new(WORKFLOW_DIRECTORY).join(format!("codemod-publish-{slug}-{hash}.yml"))
}

fn yaml_string(value: &str) -> String {
    serde_json::to_string(value).expect("strings are always JSON serializable")
}

fn render_workflow(
    package_name: &str,
    repository_directory: Option<&str>,
    default_branch: &str,
    hash: &str,
) -> String {
    let directory = repository_directory.unwrap_or(".");
    let package_json = repository_directory
        .map(|path| format!("{path}/package.json"))
        .unwrap_or_else(|| "package.json".to_string());
    let path_glob = repository_directory
        .map(|path| format!("{path}/**"))
        .unwrap_or_else(|| "**".to_string());

    WORKFLOW_PLACEHOLDER
        .get_or_init(|| {
            Regex::new(
                r"__(?:PACKAGE_NAME|DEFAULT_BRANCH|PACKAGE_PATH_GLOB|PACKAGE_HASH|PACKAGE_JSON_PATH|PACKAGE_DIRECTORY)__",
            )
            .expect("workflow placeholder regex is valid")
        })
        .replace_all(
            WORKFLOW_TEMPLATE,
            |captures: &Captures<'_>| match &captures[0] {
                "__PACKAGE_NAME__" => package_name.to_string(),
                "__DEFAULT_BRANCH__" => yaml_string(default_branch),
                "__PACKAGE_PATH_GLOB__" => yaml_string(&path_glob),
                "__PACKAGE_HASH__" => hash.to_string(),
                "__PACKAGE_JSON_PATH__" => yaml_string(&package_json),
                "__PACKAGE_DIRECTORY__" => yaml_string(directory),
                placeholder => unreachable!("unrecognized workflow placeholder: {placeholder}"),
            },
        )
        .into_owned()
}

fn write_managed_workflow(path: &Path, package_name: &str, content: &[u8]) -> Result<bool> {
    let marker = format!("# Managed by Codemod Builder for {package_name}.");
    match fs::read(path) {
        Ok(existing) if existing == content => return Ok(false),
        Ok(existing) if !existing.starts_with(marker.as_bytes()) => {
            bail!("Refusing to replace unmanaged workflow {}", path.display());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to read existing workflow {}", path.display()));
        }
    }

    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("Workflow path has no parent directory"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create workflow directory {}", parent.display()))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".codemod-publish.")
        .tempfile_in(parent)
        .with_context(|| {
            format!(
                "Failed to create a temporary workflow in {}",
                parent.display()
            )
        })?;
    temporary
        .write_all(content)
        .context("Failed to write the temporary workflow")?;
    temporary
        .as_file_mut()
        .sync_all()
        .context("Failed to flush the temporary workflow")?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("Failed to replace workflow {}", path.display()))?;
    Ok(true)
}

fn path_with_forward_slashes(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn renders_nested_default_branch_workflow() {
        let content = render_workflow(
            "@acme/rename-foo",
            Some("codemods/rename-foo"),
            "main",
            "12345678",
        );

        assert!(content.contains("branches:\n      - \"main\""));
        assert!(content.contains("- \"codemods/rename-foo/**\""));
        assert!(content.contains("PACKAGE_JSON: \"codemods/rename-foo/package.json\""));
        assert!(content.contains("path: \"codemods/rename-foo\""));
        assert!(content.contains("if: steps.version.outputs.changed == 'true'"));
    }

    #[test]
    fn renders_repository_root_workflow() {
        let content = render_workflow("rename-foo", None, "trunk", "12345678");

        assert!(content.contains("- \"**\""));
        assert!(content.contains("PACKAGE_JSON: \"package.json\""));
        assert!(content.contains("path: \".\""));
    }

    #[test]
    fn renders_placeholder_like_values_without_cascading_replacements() {
        let content = render_workflow(
            "__PACKAGE_HASH__",
            Some("codemods/__DEFAULT_BRANCH__"),
            "__PACKAGE_DIRECTORY__",
            "12345678",
        );

        assert!(content.contains("# Managed by Codemod Builder for __PACKAGE_HASH__."));
        assert!(content.contains("branches:\n      - \"__PACKAGE_DIRECTORY__\""));
        assert!(content.contains("PACKAGE_JSON: \"codemods/__DEFAULT_BRANCH__/package.json\""));
        assert!(content.contains("path: \"codemods/__DEFAULT_BRANCH__\""));
    }

    #[test]
    fn workflow_paths_are_stable_and_package_specific() {
        let first_hash = package_hash("@acme/rename-foo");
        let first = workflow_path("@acme/rename-foo", &first_hash);
        let repeated = workflow_path("@acme/rename-foo", &first_hash);
        let second_hash = package_hash("@other/rename-foo");
        let second = workflow_path("@other/rename-foo", &second_hash);

        assert_eq!(first, repeated);
        assert_ne!(first, second);
        assert!(
            path_with_forward_slashes(&first)
                .starts_with(".github/workflows/codemod-publish-acme-rename-foo-")
        );
    }

    #[test]
    fn managed_writes_are_idempotent_and_reject_unmanaged_collisions() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(".github/workflows/publish.yml");
        let content = b"# Managed by Codemod Builder for rename-foo.\nworkflow\n";

        assert!(write_managed_workflow(&path, "rename-foo", content).unwrap());
        assert!(!write_managed_workflow(&path, "rename-foo", content).unwrap());
        fs::write(&path, "user workflow\n").unwrap();
        assert!(write_managed_workflow(&path, "rename-foo", content).is_err());
    }

    #[test]
    fn managed_writes_propagate_existing_workflow_read_errors() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(".github/workflows/publish.yml");
        fs::create_dir_all(&path).unwrap();

        let error = write_managed_workflow(&path, "rename-foo", b"workflow\n").unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Failed to read existing workflow")
        );
    }

    #[test]
    fn rejects_invalid_package_names_and_branches() {
        assert!(validate_package_name("@acme/rename-foo").is_ok());
        assert!(validate_package_name("rename foo").is_err());
        assert!(validate_default_branch("feature/source-sync").is_ok());
        assert!(validate_default_branch("main\nother").is_err());
    }
}
