use super::types::{
    AutoSafeApplyResult, ComponentReconcileDecision, ManagedUpdateManifestComponent,
    ReconcileDecisionStatus, UpdatePolicyContext, UpdatePolicyMode, CURRENT_CLI_VERSION,
};
use crate::commands::harness_adapter::{
    Harness, ManagedComponentSnapshot, ManagedStateWriteResult, ManagedStateWriteStatus,
};
use std::cmp::Ordering;
use std::collections::HashMap;

pub(in crate::commands::ai) fn build_component_reconcile_decisions(
    context: &UpdatePolicyContext,
    harness: Harness,
    managed_components: &[ManagedComponentSnapshot],
) -> Vec<ComponentReconcileDecision> {
    if managed_components.is_empty() {
        return Vec::new();
    }

    let mut decisions = Vec::new();

    if let Some(remote_snapshot) = &context.remote_manifest {
        let mut remote_by_id = remote_snapshot
            .manifest
            .components
            .iter()
            .map(|component| (component.id.as_str(), component))
            .collect::<HashMap<_, _>>();

        for local_component in managed_components {
            let decision = match remote_by_id.remove(local_component.id.as_str()) {
                Some(remote_component) => {
                    reconcile_component(local_component, remote_component, harness)
                }
                None => ComponentReconcileDecision {
                    id: local_component.id.clone(),
                    kind: local_component.kind.as_str().to_string(),
                    local_version: local_component.version.clone(),
                    remote_version: None,
                    status: ReconcileDecisionStatus::Unverifiable,
                    reason: "remote_component_missing".to_string(),
                },
            };
            decisions.push(decision);
        }

        for remote_component in remote_by_id.into_values() {
            if let Some(decision) = reconcile_remote_only_component(remote_component, harness) {
                decisions.push(decision);
            }
        }
    } else {
        let reason = if context.mode == UpdatePolicyMode::Manual {
            "remote_manifest_not_requested"
        } else {
            "remote_manifest_unavailable"
        };
        for local_component in managed_components {
            decisions.push(ComponentReconcileDecision {
                id: local_component.id.clone(),
                kind: local_component.kind.as_str().to_string(),
                local_version: local_component.version.clone(),
                remote_version: None,
                status: ReconcileDecisionStatus::Unverifiable,
                reason: reason.to_string(),
            });
        }
    }

    decisions.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.id.cmp(&right.id))
    });
    decisions
}

fn reconcile_component(
    local_component: &ManagedComponentSnapshot,
    remote_component: &ManagedUpdateManifestComponent,
    harness: Harness,
) -> ComponentReconcileDecision {
    if remote_component.kind.trim() != local_component.kind.as_str() {
        return ComponentReconcileDecision {
            id: local_component.id.clone(),
            kind: local_component.kind.as_str().to_string(),
            local_version: local_component.version.clone(),
            remote_version: Some(remote_component.version.clone()),
            status: ReconcileDecisionStatus::Incompatible,
            reason: format!(
                "kind_mismatch(local={},remote={})",
                local_component.kind.as_str(),
                remote_component.kind.trim()
            ),
        };
    }

    if let Some(reason) = harness_compatibility_reason(remote_component, harness) {
        return ComponentReconcileDecision {
            id: local_component.id.clone(),
            kind: local_component.kind.as_str().to_string(),
            local_version: local_component.version.clone(),
            remote_version: Some(remote_component.version.clone()),
            status: ReconcileDecisionStatus::Incompatible,
            reason,
        };
    }

    match cli_version_compatibility(remote_component) {
        CliVersionCompatibility::Compatible => {}
        CliVersionCompatibility::Incompatible(reason) => {
            return ComponentReconcileDecision {
                id: local_component.id.clone(),
                kind: local_component.kind.as_str().to_string(),
                local_version: local_component.version.clone(),
                remote_version: Some(remote_component.version.clone()),
                status: ReconcileDecisionStatus::Incompatible,
                reason,
            };
        }
        CliVersionCompatibility::Unverifiable(reason) => {
            return ComponentReconcileDecision {
                id: local_component.id.clone(),
                kind: local_component.kind.as_str().to_string(),
                local_version: local_component.version.clone(),
                remote_version: Some(remote_component.version.clone()),
                status: ReconcileDecisionStatus::Unverifiable,
                reason,
            };
        }
    }

    match (
        local_component.version.as_deref(),
        remote_component.version.trim().is_empty(),
    ) {
        (Some(local_version), false) => {
            match compare_semver_like(local_version, remote_component.version.trim()) {
                Some(Ordering::Equal) => ComponentReconcileDecision {
                    id: local_component.id.clone(),
                    kind: local_component.kind.as_str().to_string(),
                    local_version: Some(local_version.to_string()),
                    remote_version: Some(remote_component.version.clone()),
                    status: ReconcileDecisionStatus::UpToDate,
                    reason: "versions_match".to_string(),
                },
                Some(Ordering::Less) => ComponentReconcileDecision {
                    id: local_component.id.clone(),
                    kind: local_component.kind.as_str().to_string(),
                    local_version: Some(local_version.to_string()),
                    remote_version: Some(remote_component.version.clone()),
                    status: ReconcileDecisionStatus::UpdateAvailable,
                    reason: "remote_version_newer".to_string(),
                },
                Some(Ordering::Greater) => ComponentReconcileDecision {
                    id: local_component.id.clone(),
                    kind: local_component.kind.as_str().to_string(),
                    local_version: Some(local_version.to_string()),
                    remote_version: Some(remote_component.version.clone()),
                    status: ReconcileDecisionStatus::Unverifiable,
                    reason: "local_version_newer_than_remote".to_string(),
                },
                None => ComponentReconcileDecision {
                    id: local_component.id.clone(),
                    kind: local_component.kind.as_str().to_string(),
                    local_version: Some(local_version.to_string()),
                    remote_version: Some(remote_component.version.clone()),
                    status: ReconcileDecisionStatus::Unverifiable,
                    reason: "version_not_comparable".to_string(),
                },
            }
        }
        _ => ComponentReconcileDecision {
            id: local_component.id.clone(),
            kind: local_component.kind.as_str().to_string(),
            local_version: local_component.version.clone(),
            remote_version: Some(remote_component.version.clone()),
            status: ReconcileDecisionStatus::Unverifiable,
            reason: "local_version_unknown".to_string(),
        },
    }
}

fn reconcile_remote_only_component(
    remote_component: &ManagedUpdateManifestComponent,
    harness: Harness,
) -> Option<ComponentReconcileDecision> {
    if let Some(reason) = harness_compatibility_reason(remote_component, harness) {
        if reason.starts_with("harness_not_applicable") {
            return None;
        }
        return Some(ComponentReconcileDecision {
            id: remote_component.id.clone(),
            kind: remote_component.kind.clone(),
            local_version: None,
            remote_version: Some(remote_component.version.clone()),
            status: ReconcileDecisionStatus::Incompatible,
            reason,
        });
    }

    match cli_version_compatibility(remote_component) {
        CliVersionCompatibility::Compatible => Some(ComponentReconcileDecision {
            id: remote_component.id.clone(),
            kind: remote_component.kind.clone(),
            local_version: None,
            remote_version: Some(remote_component.version.clone()),
            status: ReconcileDecisionStatus::UpdateAvailable,
            reason: "component_missing_locally".to_string(),
        }),
        CliVersionCompatibility::Incompatible(reason) => Some(ComponentReconcileDecision {
            id: remote_component.id.clone(),
            kind: remote_component.kind.clone(),
            local_version: None,
            remote_version: Some(remote_component.version.clone()),
            status: ReconcileDecisionStatus::Incompatible,
            reason,
        }),
        CliVersionCompatibility::Unverifiable(reason) => Some(ComponentReconcileDecision {
            id: remote_component.id.clone(),
            kind: remote_component.kind.clone(),
            local_version: None,
            remote_version: Some(remote_component.version.clone()),
            status: ReconcileDecisionStatus::Unverifiable,
            reason,
        }),
    }
}

fn harness_compatibility_reason(
    component: &ManagedUpdateManifestComponent,
    harness: Harness,
) -> Option<String> {
    component.harnesses.as_ref().and_then(|harnesses| {
        if harnesses.is_empty() {
            return Some("harness_not_applicable(empty_harnesses)".to_string());
        }
        let harness_value = harness.as_str();
        let applies = harnesses.iter().any(|entry| {
            let normalized = entry.trim().to_ascii_lowercase();
            normalized == harness_value || normalized == "*" || normalized == "all"
        });
        if applies {
            None
        } else {
            Some(format!(
                "harness_not_applicable(target={},supported={})",
                harness_value,
                harnesses.join(",")
            ))
        }
    })
}

enum CliVersionCompatibility {
    Compatible,
    Incompatible(String),
    Unverifiable(String),
}

fn cli_version_compatibility(
    component: &ManagedUpdateManifestComponent,
) -> CliVersionCompatibility {
    if let Some(min_cli_version) = component.min_cli_version.as_deref() {
        let min_cli_version = min_cli_version.trim();
        match compare_semver_like(CURRENT_CLI_VERSION, min_cli_version) {
            Some(Ordering::Less) => {
                return CliVersionCompatibility::Incompatible(format!(
                    "cli_version_below_min(current={},min={})",
                    CURRENT_CLI_VERSION, min_cli_version
                ));
            }
            Some(_) => {}
            None => {
                return CliVersionCompatibility::Unverifiable(format!(
                    "min_cli_version_not_comparable(value={})",
                    min_cli_version
                ));
            }
        }
    }

    if let Some(max_cli_version) = component.max_cli_version.as_deref() {
        let max_cli_version = max_cli_version.trim();
        match compare_semver_like(CURRENT_CLI_VERSION, max_cli_version) {
            Some(Ordering::Greater) => {
                return CliVersionCompatibility::Incompatible(format!(
                    "cli_version_above_max(current={},max={})",
                    CURRENT_CLI_VERSION, max_cli_version
                ));
            }
            Some(_) => {}
            None => {
                return CliVersionCompatibility::Unverifiable(format!(
                    "max_cli_version_not_comparable(value={})",
                    max_cli_version
                ));
            }
        }
    }

    CliVersionCompatibility::Compatible
}

fn compare_semver_like(left: &str, right: &str) -> Option<Ordering> {
    let left = parse_semver_like(left)?;
    let right = parse_semver_like(right)?;
    Some(left.cmp(&right))
}

fn parse_semver_like(value: &str) -> Option<(u64, u64, u64)> {
    let core = value.trim().split(['-', '+']).next()?.trim();
    if core.is_empty() {
        return None;
    }

    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    let patch = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }

    Some((major, minor, patch))
}

pub(in crate::commands::ai) fn update_policy_behavior(
    context: &UpdatePolicyContext,
) -> &'static str {
    match context.mode {
        UpdatePolicyMode::Manual => "reconcile_on_install",
        UpdatePolicyMode::Notify if context.fallback_applied => {
            "notify_on_local_state_change_fallback"
        }
        UpdatePolicyMode::Notify => "notify_on_remote_or_local_change",
        UpdatePolicyMode::AutoSafe if context.fallback_applied => "local_auto_reconcile_fallback",
        UpdatePolicyMode::AutoSafe => "auto_reconcile_with_remote_checks",
    }
}

pub(in crate::commands::ai) fn update_policy_runtime_message(
    context: &UpdatePolicyContext,
    managed_state: Option<&ManagedStateWriteResult>,
    auto_safe_apply: Option<&AutoSafeApplyResult>,
) -> Option<String> {
    match context.mode {
        UpdatePolicyMode::Manual => None,
        UpdatePolicyMode::Notify => Some(match managed_state.map(|state| state.status) {
            Some(ManagedStateWriteStatus::Created) => {
                "Update policy notify: codemod-managed state was created in this install (local fallback active)."
                    .to_string()
            }
            Some(ManagedStateWriteStatus::Updated) => {
                "Update policy notify: codemod-managed state changed in this install (local fallback active)."
                    .to_string()
            }
            Some(ManagedStateWriteStatus::Unchanged) => {
                "Update policy notify: no codemod-managed state change detected (local fallback active)."
                    .to_string()
            }
            None => "Update policy notify: managed state is unavailable; change notifications are limited.".to_string(),
        }),
        UpdatePolicyMode::AutoSafe => match auto_safe_apply {
            Some(result)
                if result.attempted > 0
                    || result.applied > 0
                    || result.skipped > 0
                    || result.failed > 0 =>
            {
                Some(format!(
                    "Update policy auto-safe: attempted {}, applied {}, skipped {}, failed {}, rolled_back={}.",
                    result.attempted, result.applied, result.skipped, result.failed, result.rolled_back
                ))
            }
            _ => None,
        },
    }
}
