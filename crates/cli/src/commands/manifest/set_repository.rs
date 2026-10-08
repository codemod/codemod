use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, ValueEnum};
use serde::Serialize;
use serde_yaml::{Mapping, Value};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use url::Url;

const MANIFEST_FILE_NAME: &str = "codemod.yaml";

#[derive(Args, Debug)]
pub struct Command {
    /// Codemod package root containing codemod.yaml
    #[arg(long, value_name = "PACKAGE_ROOT")]
    path: PathBuf,

    /// Public source repository URL
    #[arg(long)]
    url: String,

    /// Package directory relative to the repository root; use `.` for the root
    #[arg(long)]
    directory: String,

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
struct SetRepositoryOutput<'a> {
    manifest_path: String,
    repository_url: &'a str,
    repository_directory: Option<&'a str>,
    changed: bool,
}

pub fn handler(args: &Command) -> Result<()> {
    let repository_url = normalize_repository_url(&args.url)?;
    let repository_directory = normalize_repository_directory(&args.directory)?;
    let manifest_path = args.path.join(MANIFEST_FILE_NAME);
    let changed = set_repository(
        &manifest_path,
        &repository_url,
        repository_directory.as_deref(),
    )?;

    let output = SetRepositoryOutput {
        manifest_path: manifest_path.display().to_string(),
        repository_url: &repository_url,
        repository_directory: repository_directory.as_deref(),
        changed,
    };

    match args.output {
        OutputFormat::Text => {
            let action = if changed {
                "Updated"
            } else {
                "Already up to date"
            };
            println!("{action}: {}", manifest_path.display());
        }
        OutputFormat::Json => println!("{}", serde_json::to_string(&output)?),
    }

    Ok(())
}

fn normalize_repository_url(input: &str) -> Result<String> {
    let trimmed = input.trim();
    let parsed = Url::parse(trimmed).context("Repository URL is invalid")?;

    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        bail!("Repository URL must be an HTTP(S) URL with a host");
    }
    let authority = trimmed
        .split_once("://")
        .map(|(_, remainder)| remainder.split(['/', '?', '#']).next().unwrap_or_default())
        .unwrap_or_default();
    if authority.contains('@') || !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("Repository URL must not contain credentials");
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        bail!("Repository URL must not contain a query or fragment");
    }

    Ok(trimmed.trim_end_matches('/').to_string())
}

pub(super) fn normalize_repository_directory(input: &str) -> Result<Option<String>> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed == "." {
        return Ok(None);
    }
    if trimmed.contains('\\') {
        bail!("Repository directory must use forward slashes");
    }

    let path = Path::new(trimmed);
    let mut normalized = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => normalized.push(value.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("Repository directory must be a repository-relative path without `..`");
            }
        }
    }

    if normalized.is_empty() {
        return Ok(None);
    }
    Ok(Some(normalized.join("/")))
}

fn set_repository(
    manifest_path: &Path,
    repository_url: &str,
    repository_directory: Option<&str>,
) -> Result<bool> {
    let original = fs::read_to_string(manifest_path)
        .with_context(|| format!("Failed to read manifest {}", manifest_path.display()))?;
    let mut manifest: Value = serde_yaml::from_str(&original)
        .with_context(|| format!("Failed to parse manifest {}", manifest_path.display()))?;
    let mapping = manifest
        .as_mapping_mut()
        .ok_or_else(|| anyhow!("Manifest root must be a YAML mapping"))?;

    let repository = repository_value(repository_url, repository_directory);
    let key = Value::String("repository".to_string());
    if mapping.get(&key) == Some(&repository) {
        return Ok(false);
    }
    mapping.insert(key, repository);

    let serialized = serde_yaml::to_string(&manifest)
        .with_context(|| format!("Failed to serialize manifest {}", manifest_path.display()))?;
    write_atomically(manifest_path, serialized.as_bytes())?;
    Ok(true)
}

fn repository_value(repository_url: &str, repository_directory: Option<&str>) -> Value {
    let Some(directory) = repository_directory else {
        return Value::String(repository_url.to_string());
    };

    let mut repository = Mapping::new();
    repository.insert(
        Value::String("url".to_string()),
        Value::String(repository_url.to_string()),
    );
    repository.insert(
        Value::String("directory".to_string()),
        Value::String(directory.to_string()),
    );
    Value::Mapping(repository)
}

fn write_atomically(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("Manifest path has no parent directory"))?;
    let permissions = fs::metadata(path)
        .with_context(|| format!("Failed to inspect manifest {}", path.display()))?
        .permissions();
    let mut temporary = tempfile::Builder::new()
        .prefix(".codemod.yaml.")
        .tempfile_in(parent)
        .with_context(|| {
            format!(
                "Failed to create a temporary manifest in {}",
                parent.display()
            )
        })?;
    temporary
        .as_file_mut()
        .set_permissions(permissions)
        .context("Failed to preserve manifest permissions")?;
    temporary
        .write_all(content)
        .context("Failed to write the temporary manifest")?;
    temporary
        .as_file_mut()
        .sync_all()
        .context("Failed to flush the temporary manifest")?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("Failed to replace manifest {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_manifest(directory: &Path, content: &str) -> PathBuf {
        let path = directory.join(MANIFEST_FILE_NAME);
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn sets_structured_repository_and_preserves_unrelated_yaml() {
        let directory = tempdir().unwrap();
        let path = write_manifest(
            directory.path(),
            "schemaVersion: 1.0.0\nname: sample\ncustom:\n  nested: true\nrepository: https://example.com/old\n",
        );

        assert!(
            set_repository(
                &path,
                "https://github.com/acme/codemods",
                Some("codemods/rename-foo"),
            )
            .unwrap()
        );

        let value: Value = serde_yaml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["custom"]["nested"], Value::Bool(true));
        assert_eq!(
            value["repository"]["url"],
            Value::String("https://github.com/acme/codemods".to_string())
        );
        assert_eq!(
            value["repository"]["directory"],
            Value::String("codemods/rename-foo".to_string())
        );
    }

    #[test]
    fn repeated_mutation_is_byte_for_byte_idempotent() {
        let directory = tempdir().unwrap();
        let path = write_manifest(directory.path(), "name: sample\nunknown: keep-me\n");

        assert!(
            set_repository(
                &path,
                "https://github.com/acme/codemods",
                Some("packages/foo")
            )
            .unwrap()
        );
        let first = fs::read(&path).unwrap();
        assert!(
            !set_repository(
                &path,
                "https://github.com/acme/codemods",
                Some("packages/foo")
            )
            .unwrap()
        );
        assert_eq!(fs::read(&path).unwrap(), first);
    }

    #[test]
    fn repository_root_uses_the_legacy_string_form() {
        let directory = tempdir().unwrap();
        let path = write_manifest(directory.path(), "name: sample\n");

        set_repository(&path, "https://github.com/acme/codemods", None).unwrap();

        let value: Value = serde_yaml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            value["repository"],
            Value::String("https://github.com/acme/codemods".to_string())
        );
    }

    #[test]
    fn normalizes_repository_directory() {
        assert_eq!(
            normalize_repository_directory("./packages//foo").unwrap(),
            Some("packages/foo".to_string())
        );
        assert_eq!(normalize_repository_directory(".").unwrap(), None);
        assert!(normalize_repository_directory("../foo").is_err());
        assert!(normalize_repository_directory("/foo").is_err());
        assert!(normalize_repository_directory("foo\\bar").is_err());
    }

    #[test]
    fn rejects_unsafe_repository_urls() {
        assert!(normalize_repository_url("git@github.com:acme/repo.git").is_err());
        assert!(normalize_repository_url("https://token@github.com/acme/repo").is_err());
        assert!(normalize_repository_url("https://@github.com/acme/repo").is_err());
        assert!(normalize_repository_url("https://github.com/acme/repo?token=secret").is_err());
    }
}
