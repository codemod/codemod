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
use serde::Serialize;
use tabled::settings::{object::Columns, Alignment, Modify, Style};
use tabled::{Table, Tabled};

#[derive(Serialize)]
pub(in crate::commands::agent) struct InstallSkillsOutput {
    pub(in crate::commands::agent) ok: bool,
    pub(in crate::commands::agent) harness: String,
    pub(in crate::commands::agent) scope: String,
    pub(in crate::commands::agent) installed: Vec<InstalledSkillOutput>,
    pub(in crate::commands::agent) managed_state: Option<ManagedStateOutput>,
    pub(in crate::commands::agent) update_policy: UpdatePolicyOutput,
    pub(in crate::commands::agent) warnings: Vec<String>,
}

#[derive(Serialize)]
pub(in crate::commands::agent) struct ManagedStateOutput {
    pub(in crate::commands::agent) path: String,
    pub(in crate::commands::agent) status: String,
}

#[derive(Serialize)]
pub(in crate::commands::agent) struct UpdatePolicyOutput {
    pub(in crate::commands::agent) mode: String,
    pub(in crate::commands::agent) trigger: String,
    pub(in crate::commands::agent) behavior: String,
    pub(in crate::commands::agent) remote_source: String,
    pub(in crate::commands::agent) fallback_applied: bool,
    pub(in crate::commands::agent) remote_manifest: Option<RemoteManifestOutput>,
    pub(in crate::commands::agent) component_decisions: Vec<ComponentDecisionOutput>,
    pub(in crate::commands::agent) auto_safe_apply: Option<AutoSafeApplyOutput>,
}

#[derive(Serialize)]
pub(in crate::commands::agent) struct RemoteManifestOutput {
    pub(in crate::commands::agent) source: String,
    pub(in crate::commands::agent) schema_version: String,
    pub(in crate::commands::agent) component_count: usize,
    pub(in crate::commands::agent) authenticity_verified: bool,
}

#[derive(Serialize)]
pub(in crate::commands::agent) struct ComponentDecisionOutput {
    pub(in crate::commands::agent) id: String,
    pub(in crate::commands::agent) kind: String,
    pub(in crate::commands::agent) local_version: Option<String>,
    pub(in crate::commands::agent) remote_version: Option<String>,
    pub(in crate::commands::agent) status: String,
    pub(in crate::commands::agent) reason: String,
}

#[derive(Serialize)]
pub(in crate::commands::agent) struct AutoSafeApplyOutput {
    pub(in crate::commands::agent) attempted: usize,
    pub(in crate::commands::agent) applied: usize,
    pub(in crate::commands::agent) skipped: usize,
    pub(in crate::commands::agent) failed: usize,
    pub(in crate::commands::agent) rolled_back: bool,
    pub(in crate::commands::agent) rollback_reason: Option<String>,
    pub(in crate::commands::agent) components: Vec<AutoSafeComponentOutput>,
}

#[derive(Serialize)]
pub(in crate::commands::agent) struct AutoSafeComponentOutput {
    pub(in crate::commands::agent) id: String,
    pub(in crate::commands::agent) path: String,
    pub(in crate::commands::agent) status: String,
    pub(in crate::commands::agent) reason: String,
}

#[derive(Serialize)]
pub(in crate::commands::agent) struct InstalledSkillOutput {
    pub(in crate::commands::agent) name: String,
    pub(in crate::commands::agent) path: String,
    pub(in crate::commands::agent) version: Option<String>,
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

#[derive(Tabled)]
struct ComponentDecisionRow {
    #[tabled(rename = "Component")]
    id: String,
    #[tabled(rename = "Status")]
    status: String,
    #[tabled(rename = "Local")]
    local_version: String,
    #[tabled(rename = "Remote")]
    remote_version: String,
    #[tabled(rename = "Reason")]
    reason: String,
}

#[derive(Tabled)]
struct AutoSafeComponentRow {
    #[tabled(rename = "Component")]
    id: String,
    #[tabled(rename = "Status")]
    status: String,
    #[tabled(rename = "Path")]
    path: String,
    #[tabled(rename = "Reason")]
    reason: String,
}

#[derive(Serialize)]
pub(in crate::commands::agent) struct ListSkillsOutput {
    pub(in crate::commands::agent) ok: bool,
    pub(in crate::commands::agent) harness: String,
    pub(in crate::commands::agent) skills: Vec<ListedSkillOutput>,
    pub(in crate::commands::agent) warnings: Vec<String>,
}

#[derive(Serialize)]
pub(in crate::commands::agent) struct ListedSkillOutput {
    pub(in crate::commands::agent) name: String,
    pub(in crate::commands::agent) scope: Option<String>,
    pub(in crate::commands::agent) path: String,
    pub(in crate::commands::agent) version: Option<String>,
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

pub(in crate::commands::agent) struct BuildInstallOutputInput<'a> {
    pub(in crate::commands::agent) harness: Harness,
    pub(in crate::commands::agent) scope: InstallScope,
    pub(in crate::commands::agent) installed: Vec<InstalledSkill>,
    pub(in crate::commands::agent) managed_state: Option<ManagedStateWriteResult>,
    pub(in crate::commands::agent) update_policy: &'a UpdatePolicyContext,
    pub(in crate::commands::agent) component_decisions: Vec<ComponentReconcileDecision>,
    pub(in crate::commands::agent) auto_safe_apply: Option<AutoSafeApplyResult>,
    pub(in crate::commands::agent) warnings: Vec<String>,
}

pub(in crate::commands::agent) fn build_install_output(
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
        warnings,
    } = input;

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
        managed_state: managed_state.map(|state| ManagedStateOutput {
            path: format_output_path(&state.path),
            status: state.status.as_str().to_string(),
        }),
        update_policy: build_update_policy_output(
            update_policy,
            component_decisions,
            auto_safe_apply,
        ),
        warnings,
    }
}

pub(in crate::commands::agent) fn print_install_output(
    output: &InstallSkillsOutput,
    format: OutputFormat,
) -> Result<()> {
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

pub(in crate::commands::agent) fn print_install_output_table(output: &InstallSkillsOutput) {
    println!("Harness: {}", output.harness);
    println!("Scope: {}", output.scope);
    if let Some(state) = &output.managed_state {
        println!("Managed state: {} ({})", state.path, state.status);
    } else {
        println!("Managed state: unavailable");
    }
    println!(
        "Update policy: {} (trigger: {}, source: {}, fallback: {})",
        output.update_policy.mode,
        output.update_policy.trigger,
        output.update_policy.remote_source,
        output.update_policy.fallback_applied
    );
    if let Some(remote_manifest) = &output.update_policy.remote_manifest {
        println!(
            "Remote manifest: {} (schema {}, components {}, authenticity_verified={})",
            remote_manifest.source,
            remote_manifest.schema_version,
            remote_manifest.component_count,
            remote_manifest.authenticity_verified
        );
    }
    if !output.update_policy.component_decisions.is_empty() {
        let decision_rows = output
            .update_policy
            .component_decisions
            .iter()
            .map(|decision| ComponentDecisionRow {
                id: decision.id.clone(),
                status: decision.status.clone(),
                local_version: decision
                    .local_version
                    .clone()
                    .unwrap_or_else(|| "-".to_string()),
                remote_version: decision
                    .remote_version
                    .clone()
                    .unwrap_or_else(|| "-".to_string()),
                reason: decision.reason.clone(),
            })
            .collect::<Vec<_>>();

        println!("Component decisions:");
        let mut decision_table = Table::new(decision_rows);
        decision_table
            .with(Style::rounded())
            .with(Modify::new(Columns::new(..)).with(Alignment::left()));
        println!("{decision_table}");
    }
    if let Some(auto_safe_apply) = &output.update_policy.auto_safe_apply {
        println!(
            "Auto-safe apply: attempted {}, applied {}, skipped {}, failed {}, rolled_back={}",
            auto_safe_apply.attempted,
            auto_safe_apply.applied,
            auto_safe_apply.skipped,
            auto_safe_apply.failed,
            auto_safe_apply.rolled_back
        );
        if let Some(rollback_reason) = &auto_safe_apply.rollback_reason {
            println!("Auto-safe rollback reason: {rollback_reason}");
        }
        if !auto_safe_apply.components.is_empty() {
            let rows = auto_safe_apply
                .components
                .iter()
                .map(|component| AutoSafeComponentRow {
                    id: component.id.clone(),
                    status: component.status.clone(),
                    path: component.path.clone(),
                    reason: component.reason.clone(),
                })
                .collect::<Vec<_>>();
            let mut table = Table::new(rows);
            table
                .with(Style::rounded())
                .with(Modify::new(Columns::new(..)).with(Alignment::left()));
            println!("{table}");
        }
    }

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

pub(in crate::commands::agent) fn build_list_output(
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

pub(in crate::commands::agent) fn print_list_output(
    output: &ListSkillsOutput,
    format: OutputFormat,
) -> Result<()> {
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

pub(in crate::commands::agent) fn print_list_output_table(output: &ListSkillsOutput) {
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

pub(in crate::commands::agent) fn build_update_policy_output(
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

pub(in crate::commands::agent) fn auto_safe_apply_output_from_result(
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
