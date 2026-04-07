use anyhow::{anyhow, Context};
use rmcp::{handler::server::wrapper::Parameters, model::*, schemars, tool, ErrorData as McpError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

const DEFAULT_SCAFFOLD_TIMEOUT_SECS: u64 = 180;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ScaffoldCodemodPackageRequest {
    /// Project directory name or `.` to scaffold in the current directory.
    #[serde(default = "default_current_dir")]
    pub path: String,
    /// Project name (`--name`).
    pub name: Option<String>,
    /// Git repository URL (`--git-repository-url`).
    pub git_repository_url: Option<String>,
    /// Project type (`ast-grep-js`, `hybrid`, `shell`, `ast-grep-yaml`).
    pub project_type: Option<String>,
    /// Scaffold a skill-focused package with an install-skill workflow.
    #[serde(default)]
    pub skill: bool,
    /// Also scaffold skill behavior alongside workflow files.
    #[serde(default)]
    pub with_skill: bool,
    /// Package manager (`npm`, `pnpm`, `bun`, `yarn`).
    pub package_manager: Option<String>,
    /// Target language.
    pub language: Option<String>,
    /// Project description.
    pub description: Option<String>,
    /// Author name and email.
    pub author: Option<String>,
    /// License identifier.
    pub license: Option<String>,
    /// Make package private.
    #[serde(default)]
    pub private: bool,
    /// Overwrite existing files.
    #[serde(default)]
    pub force: bool,
    /// Create GitHub Actions workflow for publishing.
    #[serde(default)]
    pub github_action: bool,
    /// Create a monorepo workspace structure.
    #[serde(default)]
    pub workspace: bool,
    /// Timeout for the scaffold command, in seconds.
    #[serde(default = "default_scaffold_timeout_seconds")]
    pub timeout_seconds: u64,
}

fn default_current_dir() -> String {
    ".".to_string()
}

fn default_scaffold_timeout_seconds() -> u64 {
    DEFAULT_SCAFFOLD_TIMEOUT_SECS
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ScaffoldCodemodPackageResponse {
    pub success: bool,
    pub package_root: String,
    pub already_existed: bool,
    pub command: Vec<String>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct PackageScaffoldHandler;

impl PackageScaffoldHandler {
    pub fn new() -> Self {
        Self
    }

    #[tool(
        description = "Scaffold a codemod package by delegating to the real `codemod init` CLI. Use this immediately after registry search shows there is no exact existing package for the requested migration. The tool mirrors actual CLI behavior and non-interactive flags instead of reimplementing scaffolding logic."
    )]
    pub async fn scaffold_codemod_package(
        &self,
        Parameters(request): Parameters<ScaffoldCodemodPackageRequest>,
    ) -> Result<CallToolResult, McpError> {
        let response = self
            .scaffold_package(request)
            .await
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;

        let content = serde_json::to_string_pretty(&response).map_err(|error| {
            McpError::internal_error(format!("Failed to serialize response: {error}"), None)
        })?;

        Ok(CallToolResult::success(vec![Content::text(content)]))
    }

    async fn scaffold_package(
        &self,
        request: ScaffoldCodemodPackageRequest,
    ) -> anyhow::Result<ScaffoldCodemodPackageResponse> {
        let package_root = canonicalize_requested_root(&request.path)?;
        let already_existed = package_root.exists();
        let invocation = cli_invocation_for_scaffold()?;
        let project_type = request
            .project_type
            .clone()
            .unwrap_or_else(|| "ast-grep-js".to_string());
        let package_manager = request.package_manager.clone().or_else(|| {
            if project_type == "ast-grep-js" || project_type == "hybrid" || request.workspace {
                Some("npm".to_string())
            } else {
                None
            }
        });
        let language = request
            .language
            .clone()
            .or_else(|| Some("typescript".to_string()));
        let description = request.description.clone().or_else(|| {
            request
                .name
                .as_ref()
                .map(|name| format!("Codemod package for {name}"))
        });
        let author = request
            .author
            .clone()
            .or_else(|| Some("Codemod".to_string()));
        let license = request.license.clone().or_else(|| Some("MIT".to_string()));

        let mut command = invocation;
        command.push("init".to_string());
        command.push(package_root.display().to_string());

        if let Some(name) = &request.name {
            command.push("--name".to_string());
            command.push(name.clone());
        }
        if let Some(url) = &request.git_repository_url {
            command.push("--git-repository-url".to_string());
            command.push(url.clone());
        }
        command.push("--project-type".to_string());
        command.push(project_type);
        if request.skill {
            command.push("--skill".to_string());
        }
        if request.with_skill {
            command.push("--with-skill".to_string());
        }
        if let Some(package_manager) = &package_manager {
            command.push("--package-manager".to_string());
            command.push(package_manager.clone());
        }
        if let Some(language) = &language {
            command.push("--language".to_string());
            command.push(language.clone());
        }
        if let Some(description) = &description {
            command.push("--description".to_string());
            command.push(description.clone());
        }
        if let Some(author) = &author {
            command.push("--author".to_string());
            command.push(author.clone());
        }
        if let Some(license) = &license {
            command.push("--license".to_string());
            command.push(license.clone());
        }
        if request.private {
            command.push("--private".to_string());
        }
        if request.force {
            command.push("--force".to_string());
        }
        if request.github_action {
            command.push("--github-action".to_string());
        }
        if request.workspace {
            command.push("--workspace".to_string());
        }
        command.push("--no-interactive".to_string());

        let executable = command
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("Missing scaffold command"))?;
        let args = command.iter().skip(1).cloned().collect::<Vec<_>>();

        let output = tokio::time::timeout(
            Duration::from_secs(request.timeout_seconds),
            Command::new(&executable).args(&args).output(),
        )
        .await;

        let response = match output {
            Ok(Ok(output)) => ScaffoldCodemodPackageResponse {
                success: output.status.success(),
                package_root: package_root.display().to_string(),
                already_existed,
                command,
                exit_code: output.status.code(),
                timed_out: false,
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                message: if already_existed {
                    Some(
                        "Target directory already existed before scaffolding. Use `force: true` only when you intentionally want to replace or repair an existing scaffold."
                            .to_string(),
                    )
                } else {
                    None
                },
            },
            Ok(Err(error)) => ScaffoldCodemodPackageResponse {
                success: false,
                package_root: package_root.display().to_string(),
                already_existed,
                command,
                exit_code: None,
                timed_out: false,
                stdout: String::new(),
                stderr: error.to_string(),
                message: if already_existed {
                    Some(
                        "The requested package directory already existed. Retry with `force: true` only if overwriting that directory is intentional."
                            .to_string(),
                    )
                } else {
                    None
                },
            },
            Err(_) => ScaffoldCodemodPackageResponse {
                success: false,
                package_root: package_root.display().to_string(),
                already_existed,
                command,
                exit_code: None,
                timed_out: true,
                stdout: String::new(),
                stderr: format!("Timed out after {}s", request.timeout_seconds),
                message: if already_existed {
                    Some(
                        "The target directory already existed before the timed-out scaffold attempt."
                            .to_string(),
                    )
                } else {
                    None
                },
            },
        };

        Ok(response)
    }
}

fn canonicalize_requested_root(path: &str) -> anyhow::Result<PathBuf> {
    let requested = PathBuf::from(path);
    let root = if requested == Path::new(".") {
        std::env::current_dir().context("Failed to determine current working directory")?
    } else if requested.is_absolute() {
        requested
    } else {
        std::env::current_dir()
            .context("Failed to determine current working directory")?
            .join(requested)
    };
    Ok(root)
}

fn cli_invocation_for_scaffold() -> anyhow::Result<Vec<String>> {
    if let Ok(command_override) = std::env::var("CODEMOD_MCP_CLI_COMMAND") {
        if !command_override.trim().is_empty() {
            let mut command = vec![command_override];
            if let Ok(args_override) = std::env::var("CODEMOD_MCP_CLI_ARGS") {
                if !args_override.trim().is_empty() {
                    let parsed = serde_json::from_str::<Vec<String>>(&args_override)
                        .context("CODEMOD_MCP_CLI_ARGS must be a JSON string array")?;
                    command.extend(parsed);
                }
            }
            return Ok(command);
        }
    }

    let current_exe = std::env::current_exe().context("Failed to resolve current executable")?;
    Ok(vec![current_exe.display().to_string()])
}

impl Default for PackageScaffoldHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{LazyLock, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    static ENV_GUARD: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn unique_temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("expected monotonic time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("codemod-mcp-scaffold-{}", unique));
        fs::create_dir_all(&dir).expect("expected temp dir");
        dir
    }

    #[tokio::test]
    async fn scaffold_tool_invokes_real_cli_shape() {
        let _guard = ENV_GUARD.lock().unwrap();
        let temp_dir = unique_temp_dir();
        let fake_cli = temp_dir.join("fake-codemod.sh");
        let args_file = temp_dir.join("args.txt");

        fs::write(
            &fake_cli,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nmkdir -p '{}'\ntouch '{}/codemod.yaml'\n",
                args_file.display(),
                temp_dir.join("generated").display(),
                temp_dir.join("generated").display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&fake_cli).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake_cli, permissions).unwrap();

        std::env::set_var("CODEMOD_MCP_CLI_COMMAND", fake_cli.display().to_string());
        std::env::remove_var("CODEMOD_MCP_CLI_ARGS");

        let handler = PackageScaffoldHandler::new();
        let response = handler
            .scaffold_package(ScaffoldCodemodPackageRequest {
                path: temp_dir.join("generated").display().to_string(),
                name: Some("example".to_string()),
                git_repository_url: None,
                project_type: Some("ast-grep-js".to_string()),
                skill: false,
                with_skill: false,
                package_manager: Some("npm".to_string()),
                language: Some("typescript".to_string()),
                description: Some("desc".to_string()),
                author: Some("Author".to_string()),
                license: Some("MIT".to_string()),
                private: false,
                force: true,
                github_action: false,
                workspace: false,
                timeout_seconds: 5,
            })
            .await
            .expect("expected scaffold response");

        let args = fs::read_to_string(args_file).unwrap();
        assert!(response.success);
        assert!(args.contains("init"));
        assert!(args.contains("generated"));
        assert!(args.contains("--name"));
        assert!(args.contains("example"));
        assert!(args.contains("--project-type"));
        assert!(args.contains("ast-grep-js"));
        assert!(args.contains("--package-manager"));
        assert!(args.contains("npm"));
        assert!(args.contains("--language"));
        assert!(args.contains("typescript"));
        assert!(args.contains("--no-interactive"));

        std::env::remove_var("CODEMOD_MCP_CLI_COMMAND");
        std::env::remove_var("CODEMOD_MCP_CLI_ARGS");
        fs::remove_dir_all(temp_dir).unwrap();
    }
}
