use crate::commands::harness_adapter::{
    resolve_adapter, resolve_install_scope, Harness, HarnessAdapterError, InstallRequest,
    InstallScope, InstalledSkill, OutputFormat, VerificationStatus,
};
use anyhow::Result;
use clap::{Args, Subcommand};
use inquire::{Confirm, Select};
use serde::Serialize;
use std::fmt;
use std::io::IsTerminal;
use tabled::settings::{object::Columns, Alignment, Modify, Style};
use tabled::{Table, Tabled};

#[derive(Args, Debug)]
pub struct Command {
    #[command(subcommand)]
    action: AgentAction,
}

#[derive(Subcommand, Debug)]
enum AgentAction {
    /// Install MCS and baseline codemod skills into harness-specific paths
    Install(InstallCommand),
    /// List installed codemod skills for a harness
    List(ListCommand),
}

#[derive(Args, Debug)]
struct InstallCommand {
    /// Target harness adapter
    #[arg(long, value_enum, default_value_t = Harness::Auto)]
    harness: Harness,
    /// Prompt for missing install options in an interactive wizard
    #[arg(long)]
    interactive: bool,
    /// Install into current repo workspace
    #[arg(long, conflicts_with = "user")]
    project: bool,
    /// Install into user-level skills path
    #[arg(long, conflicts_with = "project")]
    user: bool,
    /// Overwrite existing skill files
    #[arg(long)]
    force: bool,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
struct ListCommand {
    /// Target harness adapter
    #[arg(long, value_enum, default_value_t = Harness::Auto)]
    harness: Harness,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,
}

pub async fn handler(args: &Command) -> Result<()> {
    match &args.action {
        AgentAction::Install(command) => {
            let install_inputs = resolve_install_inputs(command).unwrap_or_else(|error| {
                exit_adapter_error(error, command.format);
            });
            let resolved_adapter =
                resolve_adapter(install_inputs.harness).unwrap_or_else(|error| {
                    exit_adapter_error(error, command.format);
                });
            let _ = resolved_adapter.adapter.metadata();
            let request = InstallRequest {
                scope: install_inputs.scope,
                force: install_inputs.force,
            };
            let installed = resolved_adapter
                .adapter
                .install_skills(&request)
                .unwrap_or_else(|error| {
                    exit_adapter_error(error, command.format);
                });
            let verification_checks =
                resolved_adapter
                    .adapter
                    .verify_skills()
                    .unwrap_or_else(|error| {
                        exit_adapter_error(error, command.format);
                    });

            if let Some(failed_check) = verification_checks
                .iter()
                .find(|check| check.status == VerificationStatus::Fail)
            {
                let reason = failed_check
                    .reason
                    .as_ref()
                    .map(|text| format!(": {text}"))
                    .unwrap_or_default();
                exit_adapter_error(
                    HarnessAdapterError::InvalidSkillPackage(format!(
                        "installed skill `{}` failed validation{reason}",
                        failed_check.skill
                    )),
                    command.format,
                );
            }

            let output = build_install_output(
                resolved_adapter.harness,
                install_inputs.scope,
                installed,
                resolved_adapter.warnings,
            );
            print_install_output(&output, command.format)?;
            Ok(())
        }
        AgentAction::List(command) => {
            let resolved_adapter = resolve_adapter(command.harness).unwrap_or_else(|error| {
                exit_adapter_error(error, command.format);
            });
            let _ = resolved_adapter.adapter.metadata();
            let listed_skills = resolved_adapter
                .adapter
                .list_skills()
                .unwrap_or_else(|error| {
                    exit_adapter_error(error, command.format);
                });
            let output = build_list_output(
                resolved_adapter.harness,
                listed_skills,
                resolved_adapter.warnings,
            );
            print_list_output(&output, command.format)?;
            Ok(())
        }
    }
}

#[derive(Serialize)]
struct InstallSkillsOutput {
    ok: bool,
    harness: String,
    scope: String,
    installed: Vec<InstalledSkillOutput>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct InstalledSkillOutput {
    name: String,
    path: String,
    version: Option<String>,
}

#[derive(Serialize)]
struct InstallErrorEnvelope {
    ok: bool,
    code: String,
    exit_code: i32,
    message: String,
    hint: String,
}

#[derive(Tabled)]
struct InstalledSkillRow {
    #[tabled(rename = "Skill")]
    name: String,
    #[tabled(rename = "Version")]
    version: String,
    #[tabled(rename = "Path")]
    path: String,
}

#[derive(Serialize)]
struct ListSkillsOutput {
    ok: bool,
    harness: String,
    skills: Vec<ListedSkillOutput>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct ListedSkillOutput {
    name: String,
    scope: Option<String>,
    path: String,
    version: Option<String>,
}

#[derive(Tabled)]
struct ListedSkillRow {
    #[tabled(rename = "Skill")]
    name: String,
    #[tabled(rename = "Scope")]
    scope: String,
    #[tabled(rename = "Version")]
    version: String,
    #[tabled(rename = "Path")]
    path: String,
}

fn build_install_output(
    harness: Harness,
    scope: InstallScope,
    installed: Vec<InstalledSkill>,
    warnings: Vec<String>,
) -> InstallSkillsOutput {
    InstallSkillsOutput {
        ok: true,
        harness: harness.as_str().to_string(),
        scope: scope.as_str().to_string(),
        installed: installed
            .into_iter()
            .map(|skill| InstalledSkillOutput {
                name: skill.name,
                path: format_output_path(&skill.path),
                version: skill.version,
            })
            .collect(),
        warnings,
    }
}

fn print_install_output(output: &InstallSkillsOutput, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(output)?);
        }
        OutputFormat::Yaml => {
            println!("{}", serde_yaml::to_string(output)?);
        }
        OutputFormat::Table => {
            print_install_output_table(output);
        }
    }

    Ok(())
}

fn print_install_output_table(output: &InstallSkillsOutput) {
    println!("Harness: {}", output.harness);
    println!("Scope: {}", output.scope);

    if !output.warnings.is_empty() {
        println!("Warnings:");
        for warning in &output.warnings {
            println!("  - {warning}");
        }
    }

    if output.installed.is_empty() {
        println!("No skills were installed.");
        return;
    }

    let rows = output
        .installed
        .iter()
        .map(|installed_skill| InstalledSkillRow {
            name: installed_skill.name.clone(),
            version: installed_skill
                .version
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            path: installed_skill.path.clone(),
        })
        .collect::<Vec<_>>();

    let mut table = Table::new(rows);
    table
        .with(Style::rounded())
        .with(Modify::new(Columns::new(..)).with(Alignment::left()));
    println!("{table}");
}

fn build_list_output(
    harness: Harness,
    listed_skills: Vec<InstalledSkill>,
    warnings: Vec<String>,
) -> ListSkillsOutput {
    ListSkillsOutput {
        ok: true,
        harness: harness.as_str().to_string(),
        skills: listed_skills
            .into_iter()
            .map(|skill| ListedSkillOutput {
                name: skill.name,
                scope: skill.scope.map(|scope| scope.as_str().to_string()),
                path: format_output_path(&skill.path),
                version: skill.version,
            })
            .collect(),
        warnings,
    }
}

fn print_list_output(output: &ListSkillsOutput, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(output)?);
        }
        OutputFormat::Yaml => {
            println!("{}", serde_yaml::to_string(output)?);
        }
        OutputFormat::Table => {
            print_list_output_table(output);
        }
    }

    Ok(())
}

fn print_list_output_table(output: &ListSkillsOutput) {
    println!("Harness: {}", output.harness);
    if !output.warnings.is_empty() {
        println!("Warnings:");
        for warning in &output.warnings {
            println!("  - {warning}");
        }
    }

    if output.skills.is_empty() {
        println!("No codemod skills found.");
        return;
    }

    let rows = output
        .skills
        .iter()
        .map(|skill| ListedSkillRow {
            name: skill.name.clone(),
            scope: skill.scope.clone().unwrap_or_else(|| "unknown".to_string()),
            version: skill
                .version
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            path: skill.path.clone(),
        })
        .collect::<Vec<_>>();

    let mut table = Table::new(rows);
    table
        .with(Style::rounded())
        .with(Modify::new(Columns::new(..)).with(Alignment::left()));
    println!("{table}");
}

fn exit_adapter_error(error: HarnessAdapterError, format: OutputFormat) -> ! {
    let envelope = InstallErrorEnvelope {
        ok: false,
        code: error.code().to_string(),
        exit_code: error.exit_code(),
        message: error.to_string(),
        hint: error.hint().to_string(),
    };

    match format {
        OutputFormat::Json => match serde_json::to_string_pretty(&envelope) {
            Ok(json) => println!("{json}"),
            Err(_) => eprintln!("{}: {}", envelope.code, envelope.message),
        },
        OutputFormat::Yaml => match serde_yaml::to_string(&envelope) {
            Ok(yaml) => println!("{yaml}"),
            Err(_) => eprintln!("{}: {}", envelope.code, envelope.message),
        },
        OutputFormat::Table => {
            eprintln!("Error [{}]: {}", envelope.code, envelope.message);
            eprintln!("Hint: {}", envelope.hint);
        }
    }

    std::process::exit(envelope.exit_code);
}

fn format_output_path(path: &std::path::Path) -> String {
    if let Ok(current_dir) = std::env::current_dir() {
        if let Ok(relative_path) = path.strip_prefix(current_dir) {
            return relative_path.display().to_string();
        }
    }

    if let Some(home_dir) = dirs::home_dir() {
        if let Ok(home_relative_path) = path.strip_prefix(home_dir) {
            return format!("~/{}", home_relative_path.display());
        }
    }

    path.display().to_string()
}

#[derive(Clone, Copy)]
struct InstallInputs {
    harness: Harness,
    scope: InstallScope,
    force: bool,
}

#[derive(Clone, Copy)]
struct HarnessPromptOption {
    harness: Harness,
    label: &'static str,
}

impl fmt::Display for HarnessPromptOption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label)
    }
}

#[derive(Clone, Copy)]
struct ScopePromptOption {
    scope: InstallScope,
    label: &'static str,
}

impl fmt::Display for ScopePromptOption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label)
    }
}

fn resolve_install_inputs(
    command: &InstallCommand,
) -> std::result::Result<InstallInputs, HarnessAdapterError> {
    if !command.interactive {
        let scope = resolve_install_scope(command.project, command.user)?;
        return Ok(InstallInputs {
            harness: command.harness,
            scope,
            force: command.force,
        });
    }

    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(HarnessAdapterError::InstallFailed(
            "--interactive requires a TTY terminal; re-run without --interactive in CI/headless environments".to_string(),
        ));
    }

    let harness = if command.harness != Harness::Auto {
        command.harness
    } else {
        let options = vec![
            HarnessPromptOption {
                harness: Harness::Auto,
                label: "auto (recommended)",
            },
            HarnessPromptOption {
                harness: Harness::Claude,
                label: "claude",
            },
            HarnessPromptOption {
                harness: Harness::Goose,
                label: "goose",
            },
            HarnessPromptOption {
                harness: Harness::Opencode,
                label: "opencode",
            },
            HarnessPromptOption {
                harness: Harness::Cursor,
                label: "cursor",
            },
        ];

        Select::new("Choose harness adapter:", options)
            .with_starting_cursor(0)
            .prompt()
            .map_err(|error| {
                HarnessAdapterError::InstallFailed(format!(
                    "interactive harness prompt failed: {error}"
                ))
            })?
            .harness
    };

    let scope = if command.project || command.user {
        resolve_install_scope(command.project, command.user)?
    } else {
        let options = vec![
            ScopePromptOption {
                scope: InstallScope::Project,
                label: "project (current workspace)",
            },
            ScopePromptOption {
                scope: InstallScope::User,
                label: "user (~/.<harness>/skills)",
            },
        ];

        Select::new("Choose install scope:", options)
            .with_starting_cursor(0)
            .prompt()
            .map_err(|error| {
                HarnessAdapterError::InstallFailed(format!(
                    "interactive scope prompt failed: {error}"
                ))
            })?
            .scope
    };

    let force = if command.force {
        true
    } else {
        Confirm::new("Overwrite existing skill files if they already exist?")
            .with_default(false)
            .prompt()
            .map_err(|error| {
                HarnessAdapterError::InstallFailed(format!(
                    "interactive overwrite prompt failed: {error}"
                ))
            })?
    };

    Ok(InstallInputs {
        harness,
        scope,
        force,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::path::PathBuf;

    #[test]
    fn install_output_json_includes_codemod_mcp_entry() {
        let output = build_install_output(
            Harness::Claude,
            InstallScope::Project,
            vec![
                InstalledSkill {
                    name: "codemod-cli".to_string(),
                    path: PathBuf::from("/tmp/.claude/skills/codemod-cli/SKILL.md"),
                    version: Some("1.0.0".to_string()),
                    scope: Some(InstallScope::Project),
                },
                InstalledSkill {
                    name: "codemod-mcp".to_string(),
                    path: PathBuf::from("/tmp/.mcp.json"),
                    version: None,
                    scope: Some(InstallScope::Project),
                },
            ],
            Vec::new(),
        );

        let output_json = serde_json::to_value(&output).expect("install output should serialize");
        let installed = output_json
            .get("installed")
            .and_then(Value::as_array)
            .expect("installed should be an array");

        let codemod_mcp = installed
            .iter()
            .find(|entry| entry.get("name").and_then(Value::as_str) == Some("codemod-mcp"))
            .expect("expected codemod-mcp installed entry");

        assert_eq!(output_json.get("ok").and_then(Value::as_bool), Some(true));
        assert_eq!(
            output_json.get("harness").and_then(Value::as_str),
            Some("claude")
        );
        assert_eq!(
            output_json.get("scope").and_then(Value::as_str),
            Some("project")
        );
        assert!(codemod_mcp.get("path").and_then(Value::as_str).is_some());
        assert!(codemod_mcp.get("version").is_some_and(Value::is_null));
    }
}
