use super::reconcile::update_policy_behavior;
use super::types::{
    AutoSafeApplyResult, ComponentReconcileDecision, UpdatePolicyContext,
    MANAGED_UPDATE_POLICY_TRIGGER,
};
use crate::commands::harness_adapter::{
    Harness, InstallScope, InstalledSkill, ManagedStateWriteResult, OutputFormat,
};
use crate::commands::output::format_output_path;
use anyhow::Result;
use console::style;
use serde::Serialize;
use std::fs;
use std::path::Path;
use tabled::settings::{object::Columns, Alignment, Modify, Style};
use tabled::{Table, Tabled};

#[derive(Serialize)]
pub(in crate::commands::ai) struct InstallSkillsOutput {
    pub(in crate::commands::ai) ok: bool,
    pub(in crate::commands::ai) harness: String,
    pub(in crate::commands::ai) scope: String,
    pub(in crate::commands::ai) installed: Vec<InstalledSkillOutput>,
    pub(in crate::commands::ai) managed_state: Option<ManagedStateOutput>,
    pub(in crate::commands::ai) update_policy: UpdatePolicyOutput,
    pub(in crate::commands::ai) notes: Vec<String>,
    pub(in crate::commands::ai) warnings: Vec<String>,
    pub(in crate::commands::ai) restart_hint: Option<String>,
}

#[derive(Serialize)]
pub(in crate::commands::ai) struct ManagedStateOutput {
    pub(in crate::commands::ai) path: String,
    pub(in crate::commands::ai) status: String,
}

#[derive(Serialize)]
pub(in crate::commands::ai) struct UpdatePolicyOutput {
    pub(in crate::commands::ai) mode: String,
    pub(in crate::commands::ai) trigger: String,
    pub(in crate::commands::ai) behavior: String,
    pub(in crate::commands::ai) remote_source: String,
    pub(in crate::commands::ai) fallback_applied: bool,
    pub(in crate::commands::ai) remote_manifest: Option<RemoteManifestOutput>,
    pub(in crate::commands::ai) component_decisions: Vec<ComponentDecisionOutput>,
    pub(in crate::commands::ai) auto_safe_apply: Option<AutoSafeApplyOutput>,
}

#[derive(Serialize)]
pub(in crate::commands::ai) struct RemoteManifestOutput {
    pub(in crate::commands::ai) source: String,
    pub(in crate::commands::ai) schema_version: String,
    pub(in crate::commands::ai) component_count: usize,
    pub(in crate::commands::ai) authenticity_verified: bool,
}

#[derive(Serialize)]
pub(in crate::commands::ai) struct ComponentDecisionOutput {
    pub(in crate::commands::ai) id: String,
    pub(in crate::commands::ai) kind: String,
    pub(in crate::commands::ai) local_version: Option<String>,
    pub(in crate::commands::ai) remote_version: Option<String>,
    pub(in crate::commands::ai) status: String,
    pub(in crate::commands::ai) reason: String,
}

#[derive(Serialize)]
pub(in crate::commands::ai) struct AutoSafeApplyOutput {
    pub(in crate::commands::ai) attempted: usize,
    pub(in crate::commands::ai) applied: usize,
    pub(in crate::commands::ai) skipped: usize,
    pub(in crate::commands::ai) failed: usize,
    pub(in crate::commands::ai) rolled_back: bool,
    pub(in crate::commands::ai) rollback_reason: Option<String>,
    pub(in crate::commands::ai) components: Vec<AutoSafeComponentOutput>,
}

#[derive(Serialize)]
pub(in crate::commands::ai) struct AutoSafeComponentOutput {
    pub(in crate::commands::ai) id: String,
    pub(in crate::commands::ai) path: String,
    pub(in crate::commands::ai) status: String,
    pub(in crate::commands::ai) reason: String,
}

#[derive(Serialize)]
pub(in crate::commands::ai) struct InstalledSkillOutput {
    pub(in crate::commands::ai) name: String,
    pub(in crate::commands::ai) path: String,
    pub(in crate::commands::ai) version: Option<String>,
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
pub(in crate::commands::ai) struct ListSkillsOutput {
    pub(in crate::commands::ai) ok: bool,
    pub(in crate::commands::ai) harness: String,
    pub(in crate::commands::ai) skills: Vec<ListedSkillOutput>,
    pub(in crate::commands::ai) warnings: Vec<String>,
}

#[derive(Serialize)]
pub(in crate::commands::ai) struct ListedSkillOutput {
    pub(in crate::commands::ai) name: String,
    pub(in crate::commands::ai) scope: Option<String>,
    pub(in crate::commands::ai) path: String,
    pub(in crate::commands::ai) version: Option<String>,
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

pub(in crate::commands::ai) struct BuildInstallOutputInput<'a> {
    pub(in crate::commands::ai) harness: Harness,
    pub(in crate::commands::ai) scope: InstallScope,
    pub(in crate::commands::ai) installed: Vec<InstalledSkill>,
    pub(in crate::commands::ai) managed_state: Option<ManagedStateWriteResult>,
    pub(in crate::commands::ai) update_policy: &'a UpdatePolicyContext,
    pub(in crate::commands::ai) component_decisions: Vec<ComponentReconcileDecision>,
    pub(in crate::commands::ai) auto_safe_apply: Option<AutoSafeApplyResult>,
    pub(in crate::commands::ai) notes: Vec<String>,
    pub(in crate::commands::ai) warnings: Vec<String>,
    pub(in crate::commands::ai) restart_hint: Option<String>,
}

pub(in crate::commands::ai) fn build_install_output(
    input: BuildInstallOutputInput<'_>,
) -> InstallSkillsOutput {
    let BuildInstallOutputInput {
        harness,
        scope,
        installed,
        managed_state,
        update_policy,
        component_decisions,
        auto_safe_apply,
        notes,
        warnings,
        restart_hint,
    } = input;

    InstallSkillsOutput {
        ok: true,
        harness: harness.as_str().to_string(),
        scope: scope.as_str().to_string(),
        installed: installed
            .into_iter()
            .map(|skill| InstalledSkillOutput {
                version: installed_component_version_for_output(&skill),
                path: format_output_path(&skill.path),
                name: skill.name,
            })
            .collect(),
        managed_state: managed_state.map(|state| ManagedStateOutput {
            path: format_output_path(&state.path),
            status: state.status.as_str().to_string(),
        }),
        update_policy: build_update_policy_output(
            update_policy,
            component_decisions,
            auto_safe_apply,
        ),
        notes,
        warnings,
        restart_hint,
    }
}

fn installed_component_version_for_output(skill: &InstalledSkill) -> Option<String> {
    if skill.version.is_some() {
        return skill.version.clone();
    }
    if skill.name == "codemod-mcp" {
        return codemod_mcp_configured_package_version(&skill.path)
            .or_else(|| Some("latest".to_string()));
    }
    None
}

fn codemod_mcp_configured_package_version(config_path: &Path) -> Option<String> {
    let content = fs::read_to_string(config_path).ok()?;
    let root: serde_json::Value = serde_json::from_str(&content).ok()?;
    let package_arg = root
        .get("mcpServers")
        .and_then(|servers| servers.get("codemod"))
        .and_then(|entry| entry.get("args"))
        .and_then(|args| args.as_array())
        .and_then(|args| args.first())
        .and_then(|value| value.as_str())?;

    let version = package_arg.strip_prefix("codemod@")?.trim();
    if version.is_empty() {
        return None;
    }
    Some(version.to_string())
}

pub(in crate::commands::ai) fn print_install_output(
    output: &InstallSkillsOutput,
    format: OutputFormat,
) -> Result<()> {
    match format {
        OutputFormat::Logs => {
            print_install_output_logs(output);
        }
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

pub(in crate::commands::ai) fn print_install_output_logs(output: &InstallSkillsOutput) {
    println!();

    if output.installed.is_empty() {
        println!("  {} No skills were installed.", style("⚠").yellow());
    } else {
        for skill in &output.installed {
            let label = humanize_skill_name(&skill.name);
            let version_suffix = skill
                .version
                .as_deref()
                .map(|v| format!("@{v}"))
                .unwrap_or_default();
            println!(
                "  {} {} ({}{} → {})",
                style("✓").green().bold(),
                label,
                skill.name,
                version_suffix,
                style(&skill.path).dim(),
            );
        }
    }

    for note in &output.notes {
        if let Some(line) = humanize_note(note) {
            println!("  {} {line}", style("✓").green().bold());
        }
    }

    if let Some(restart_hint) = &output.restart_hint {
        println!();
        println!("  {} {}", style("→").cyan().bold(), restart_hint);
    }

    println!();
}

fn humanize_skill_name(name: &str) -> String {
    match name {
        "codemod" => "Installed Codemod skill".to_string(),
        "codemod-mcp" => "Installed Codemod MCP".to_string(),
        other => format!("Installed {other}"),
    }
}

fn humanize_note(note: &str) -> Option<String> {
    if note.starts_with("Installed codemod creation command in: ") {
        let paths = note.trim_start_matches("Installed codemod creation command in: ");
        return Some(format!(
            "Installed /codemod command ({})",
            style(paths).dim()
        ));
    }
    if note.starts_with("Updated discovery hints in: ") {
        let paths = note.trim_start_matches("Updated discovery hints in: ");
        return Some(format!(
            "Updated discovery hints in: {}",
            style(paths).dim()
        ));
    }
    // Suppress internal/noisy messages
    None
}

pub(in crate::commands::ai) fn print_install_output_table(output: &InstallSkillsOutput) {
    println!("Harness: {}", output.harness);
    println!("Scope: {}", output.scope);

    if output.installed.is_empty() {
        println!("Components installed: none");
    } else {
        let rows = output
            .installed
            .iter()
            .map(|installed_skill| InstalledSkillRow {
                name: installed_skill.name.clone(),
                version: installed_skill
                    .version
                    .clone()
                    .unwrap_or_else(|| "n/a".to_string()),
                path: installed_skill.path.clone(),
            })
            .collect::<Vec<_>>();

        println!("Components installed:");
        let mut table = Table::new(rows);
        table
            .with(Style::rounded())
            .with(Modify::new(Columns::new(..)).with(Alignment::left()));
        println!("{table}");
    }

    for note in &output.notes {
        if let Some(human_note) = humanize_note(note) {
            println!("  {human_note}");
        }
    }

    if let Some(restart_hint) = &output.restart_hint {
        println!("  {restart_hint}");
    }
}

pub(in crate::commands::ai) fn build_list_output(
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

pub(in crate::commands::ai) fn print_list_output(
    output: &ListSkillsOutput,
    format: OutputFormat,
) -> Result<()> {
    match format {
        OutputFormat::Logs => {
            print_list_output_logs(output);
        }
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

pub(in crate::commands::ai) fn print_list_output_logs(output: &ListSkillsOutput) {
    println!(
        "Found {} codemod skill(s) for `{}`",
        output.skills.len(),
        output.harness
    );

    if output.skills.is_empty() {
        println!("No codemod skills found.");
    } else {
        for skill in &output.skills {
            let scope = skill.scope.as_deref().unwrap_or("unknown");
            let version = skill.version.as_deref().unwrap_or("n/a");
            println!(
                "  - {}@{} [{}] -> {}",
                skill.name, version, scope, skill.path
            );
        }
    }

    if !output.warnings.is_empty() {
        println!("Warnings:");
        for warning in &output.warnings {
            println!("  - {warning}");
        }
    }
}

pub(in crate::commands::ai) fn print_list_output_table(output: &ListSkillsOutput) {
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
            version: skill.version.clone().unwrap_or_else(|| "n/a".to_string()),
            path: skill.path.clone(),
        })
        .collect::<Vec<_>>();

    let mut table = Table::new(rows);
    table
        .with(Style::rounded())
        .with(Modify::new(Columns::new(..)).with(Alignment::left()));
    println!("{table}");
}

pub(in crate::commands::ai) fn build_update_policy_output(
    context: &UpdatePolicyContext,
    component_decisions: Vec<ComponentReconcileDecision>,
    auto_safe_apply: Option<AutoSafeApplyResult>,
) -> UpdatePolicyOutput {
    UpdatePolicyOutput {
        mode: context.mode.as_str().to_string(),
        trigger: MANAGED_UPDATE_POLICY_TRIGGER.to_string(),
        behavior: update_policy_behavior(context).to_string(),
        remote_source: context.remote_source.clone(),
        fallback_applied: context.fallback_applied,
        remote_manifest: context
            .remote_manifest
            .as_ref()
            .map(|snapshot| RemoteManifestOutput {
                source: snapshot.source.clone(),
                schema_version: snapshot.manifest.schema_version.clone(),
                component_count: snapshot.manifest.components.len(),
                authenticity_verified: snapshot.authenticity_verified,
            }),
        component_decisions: component_decisions
            .into_iter()
            .map(|decision| ComponentDecisionOutput {
                id: decision.id,
                kind: decision.kind,
                local_version: decision.local_version,
                remote_version: decision.remote_version,
                status: decision.status.as_str().to_string(),
                reason: decision.reason,
            })
            .collect::<Vec<_>>(),
        auto_safe_apply: auto_safe_apply.map(auto_safe_apply_output_from_result),
    }
}

pub(in crate::commands::ai) fn auto_safe_apply_output_from_result(
    result: AutoSafeApplyResult,
) -> AutoSafeApplyOutput {
    AutoSafeApplyOutput {
        attempted: result.attempted,
        applied: result.applied,
        skipped: result.skipped,
        failed: result.failed,
        rolled_back: result.rolled_back,
        rollback_reason: result.rollback_reason,
        components: result
            .components
            .into_iter()
            .map(|component| AutoSafeComponentOutput {
                id: component.id,
                path: format_output_path(&component.path),
                status: component.status.as_str().to_string(),
                reason: component.reason,
            })
            .collect::<Vec<_>>(),
    }
}

#[cfg(test)]
fn should_print_component_decisions_table(update_policy: &UpdatePolicyOutput) -> bool {
    if update_policy.component_decisions.is_empty() {
        return false;
    }
    if update_policy.remote_manifest.is_some() {
        return true;
    }

    update_policy.component_decisions.iter().any(|decision| {
        !(decision.status == "unverifiable"
            && matches!(
                decision.reason.as_str(),
                "remote_manifest_not_requested" | "remote_manifest_unavailable"
            ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update_policy_with_decisions(
        component_decisions: Vec<ComponentDecisionOutput>,
        remote_manifest: Option<RemoteManifestOutput>,
    ) -> UpdatePolicyOutput {
        UpdatePolicyOutput {
            mode: "auto-safe".to_string(),
            trigger: "install_and_periodic".to_string(),
            behavior: "auto_reconcile_with_remote_checks".to_string(),
            remote_source: "registry:https://app.codemod.com/".to_string(),
            fallback_applied: false,
            remote_manifest,
            component_decisions,
            auto_safe_apply: None,
        }
    }

    fn decision(status: &str, reason: &str) -> ComponentDecisionOutput {
        ComponentDecisionOutput {
            id: "codemod".to_string(),
            kind: "skill".to_string(),
            local_version: Some("1.0.0".to_string()),
            remote_version: None,
            status: status.to_string(),
            reason: reason.to_string(),
        }
    }

    #[test]
    fn component_decision_table_hidden_when_only_manifest_missing_reasons_exist() {
        let output = update_policy_with_decisions(
            vec![
                decision("unverifiable", "remote_manifest_not_requested"),
                decision("unverifiable", "remote_manifest_unavailable"),
            ],
            None,
        );
        assert!(!should_print_component_decisions_table(&output));
    }

    #[test]
    fn component_decision_table_shown_when_remote_manifest_is_available() {
        let output = update_policy_with_decisions(
            vec![decision("up_to_date", "versions_match")],
            Some(RemoteManifestOutput {
                source: "registry:https://app.codemod.com/".to_string(),
                schema_version: "1".to_string(),
                component_count: 1,
                authenticity_verified: true,
            }),
        );
        assert!(should_print_component_decisions_table(&output));
    }

    #[test]
    fn component_decision_table_shown_when_actionable_local_decision_exists_without_manifest() {
        let output = update_policy_with_decisions(
            vec![
                decision("unverifiable", "remote_manifest_not_requested"),
                decision(
                    "incompatible",
                    "cli_version_below_min(current=1.0.0,min=2.0.0)",
                ),
            ],
            None,
        );
        assert!(should_print_component_decisions_table(&output));
    }
}
