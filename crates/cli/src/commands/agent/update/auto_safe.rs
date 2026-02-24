use super::policy::maybe_attach_registry_auth;
use super::types::{
    AutoSafeApplyExecution, AutoSafeApplyResult, AutoSafeComponentResult, AutoSafeComponentStatus,
    ComponentBackup, ComponentReconcileDecision, ManagedUpdateManifestComponent,
    ReconcileDecisionStatus, StagedComponentUpdate, StagedFileWrite, UpdatePolicyContext,
    UpdatePolicyMode, MANAGED_UPDATE_MANIFEST_REQUEST_TIMEOUT_SECS,
};
use crate::commands::harness_adapter::{ManagedComponentKind, ManagedComponentSnapshot};
use butterflow_core::utils::get_cache_dir;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::time::sleep;

const AUTO_SAFE_UPDATE_LOCK_RELATIVE_PATH: &str = "agent/managed-component-update.lock";
const AUTO_SAFE_LOCK_TIMEOUT_SECS: u64 = 3;
const AUTO_SAFE_LOCK_RETRY_MILLIS: u64 = 200;
const AUTO_SAFE_LOCK_STALE_SECS: u64 = 600;
const AUTO_SAFE_UPDATE_LOCK_PATH_ENV_VAR: &str = "CODEMOD_AGENT_UPDATE_LOCK_PATH";
const AUTO_SAFE_LOCK_TIMEOUT_MILLIS_ENV_VAR: &str = "CODEMOD_AGENT_UPDATE_LOCK_TIMEOUT_MS";
const AUTO_SAFE_LOCK_RETRY_MILLIS_ENV_VAR: &str = "CODEMOD_AGENT_UPDATE_LOCK_RETRY_MS";
const AUTO_SAFE_LOCK_STALE_SECS_ENV_VAR: &str = "CODEMOD_AGENT_UPDATE_LOCK_STALE_SECS";

#[derive(Clone, Copy, Debug)]
struct AutoSafeLockPolicy {
    timeout: Duration,
    retry_interval: Duration,
    stale_after: Duration,
}

impl AutoSafeLockPolicy {
    fn default_policy() -> Self {
        Self {
            timeout: Duration::from_secs(AUTO_SAFE_LOCK_TIMEOUT_SECS),
            retry_interval: Duration::from_millis(AUTO_SAFE_LOCK_RETRY_MILLIS),
            stale_after: Duration::from_secs(AUTO_SAFE_LOCK_STALE_SECS),
        }
    }

    fn from_environment() -> Self {
        let defaults = Self::default_policy();
        Self {
            timeout: Duration::from_millis(parse_u64_env_or_default(
                AUTO_SAFE_LOCK_TIMEOUT_MILLIS_ENV_VAR,
                defaults.timeout.as_millis() as u64,
            )),
            retry_interval: Duration::from_millis(parse_u64_env_or_default(
                AUTO_SAFE_LOCK_RETRY_MILLIS_ENV_VAR,
                defaults.retry_interval.as_millis() as u64,
            )),
            stale_after: Duration::from_secs(parse_u64_env_or_default(
                AUTO_SAFE_LOCK_STALE_SECS_ENV_VAR,
                defaults.stale_after.as_secs(),
            )),
        }
    }
}

#[derive(Clone, Deserialize, Serialize, Debug)]
struct AutoSafeLockMetadata {
    pid: u32,
    acquired_at_epoch_secs: u64,
}

#[derive(Debug)]
struct AutoSafeLockGuard {
    path: PathBuf,
    released: bool,
}

impl AutoSafeLockGuard {
    fn release(mut self) -> std::result::Result<(), String> {
        self.released = true;
        release_auto_safe_update_lock(&self.path)
    }
}

impl Drop for AutoSafeLockGuard {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let _ = release_auto_safe_update_lock(&self.path);
    }
}

#[derive(Debug)]
struct AutoSafeLockAcquireResult {
    guard: AutoSafeLockGuard,
    warnings: Vec<String>,
}

pub(in crate::commands::agent) async fn maybe_apply_auto_safe_updates(
    update_policy: &UpdatePolicyContext,
    component_decisions: &[ComponentReconcileDecision],
    managed_components: &[ManagedComponentSnapshot],
) -> AutoSafeApplyExecution {
    if update_policy.mode != UpdatePolicyMode::AutoSafe {
        return AutoSafeApplyExecution::default();
    }

    let mut execution = AutoSafeApplyExecution::default();
    let Some(remote_snapshot) = update_policy.remote_manifest.as_ref() else {
        execution.warnings.push(
            "Auto-safe update apply skipped because remote manifest is unavailable.".to_string(),
        );
        execution.result = Some(AutoSafeApplyResult {
            attempted: 0,
            applied: 0,
            skipped: 0,
            failed: 0,
            rolled_back: false,
            rollback_reason: Some("remote_manifest_unavailable".to_string()),
            components: Vec::new(),
        });
        return execution;
    };

    let attempted = component_decisions
        .iter()
        .filter(|decision| decision.status == ReconcileDecisionStatus::UpdateAvailable)
        .count();

    if attempted == 0 {
        execution.result = Some(AutoSafeApplyResult {
            attempted: 0,
            applied: 0,
            skipped: 0,
            failed: 0,
            rolled_back: false,
            rollback_reason: None,
            components: Vec::new(),
        });
        return execution;
    }

    let lock_acquire =
        match acquire_auto_safe_update_lock(AutoSafeLockPolicy::from_environment()).await {
            Ok(lock) => lock,
            Err(error) => {
                execution
                    .warnings
                    .push(format!("Auto-safe update apply skipped: {error}."));
                execution.result = Some(AutoSafeApplyResult {
                    attempted,
                    applied: 0,
                    skipped: attempted,
                    failed: 0,
                    rolled_back: false,
                    rollback_reason: Some("lock_acquire_failed".to_string()),
                    components: skipped_update_available_components(
                        component_decisions,
                        "lock_acquire_failed",
                    ),
                });
                return execution;
            }
        };
    let AutoSafeLockAcquireResult {
        guard: lock_guard,
        warnings: lock_warnings,
    } = lock_acquire;
    execution.warnings.extend(lock_warnings);

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(
            MANAGED_UPDATE_MANIFEST_REQUEST_TIMEOUT_SECS,
        ))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            execution.warnings.push(format!(
                "Auto-safe update apply skipped: failed to initialize HTTP client ({error})."
            ));
            execution.result = Some(AutoSafeApplyResult {
                attempted,
                applied: 0,
                skipped: attempted,
                failed: 0,
                rolled_back: false,
                rollback_reason: Some("http_client_init_failed".to_string()),
                components: skipped_update_available_components(
                    component_decisions,
                    "http_client_init_failed",
                ),
            });
            return execution;
        }
    };

    let local_by_id = managed_components
        .iter()
        .map(|component| (component.id.as_str(), component))
        .collect::<HashMap<_, _>>();
    let remote_by_id = remote_snapshot
        .manifest
        .components
        .iter()
        .map(|component| (component.id.as_str(), component))
        .collect::<HashMap<_, _>>();

    let mut staged_updates = Vec::new();
    let mut results = Vec::new();

    for decision in component_decisions
        .iter()
        .filter(|decision| decision.status == ReconcileDecisionStatus::UpdateAvailable)
    {
        let Some(local_component) = local_by_id.get(decision.id.as_str()) else {
            results.push(AutoSafeComponentResult {
                id: decision.id.clone(),
                path: PathBuf::new(),
                status: AutoSafeComponentStatus::Skipped,
                reason: "component_missing_locally".to_string(),
            });
            continue;
        };
        let Some(remote_component) = remote_by_id.get(decision.id.as_str()) else {
            results.push(AutoSafeComponentResult {
                id: decision.id.clone(),
                path: local_component.path.clone(),
                status: AutoSafeComponentStatus::Skipped,
                reason: "remote_component_missing".to_string(),
            });
            continue;
        };

        if !is_auto_safe_apply_supported_kind(local_component.kind) {
            results.push(AutoSafeComponentResult {
                id: decision.id.clone(),
                path: local_component.path.clone(),
                status: AutoSafeComponentStatus::Skipped,
                reason: "unsupported_component_kind_for_auto_safe_apply".to_string(),
            });
            continue;
        }

        match fetch_remote_component_bytes(&client, &update_policy.remote_source, remote_component)
            .await
        {
            Ok(bytes) => {
                match staged_component_update_from_payload(local_component, remote_component, bytes)
                {
                    Ok(update) => staged_updates.push(update),
                    Err(error) => results.push(AutoSafeComponentResult {
                        id: decision.id.clone(),
                        path: local_component.path.clone(),
                        status: AutoSafeComponentStatus::Failed,
                        reason: format!("staging_failed({error})"),
                    }),
                }
            }
            Err(error) => results.push(AutoSafeComponentResult {
                id: decision.id.clone(),
                path: local_component.path.clone(),
                status: AutoSafeComponentStatus::Failed,
                reason: format!("remote_fetch_failed({error})"),
            }),
        }
    }

    let apply_result = apply_staged_component_updates(staged_updates);
    let mut merged_components = results;
    merged_components.extend(apply_result.components);

    let summarized = summarize_auto_safe_apply_result(
        attempted,
        apply_result.rolled_back,
        apply_result.rollback_reason,
        merged_components,
    );
    if summarized.rolled_back {
        execution.warnings.push(format!(
            "Auto-safe apply rolled back changes{}.",
            summarized
                .rollback_reason
                .as_ref()
                .map(|reason| format!(" ({reason})"))
                .unwrap_or_default()
        ));
    }
    if summarized.failed > 0 {
        execution.warnings.push(format!(
            "Auto-safe apply encountered {} failed component update(s).",
            summarized.failed
        ));
    }

    execution.result = Some(summarized);
    if let Err(error) = lock_guard.release() {
        execution.warnings.push(format!(
            "Auto-safe update lock release failed ({error}); stale lock may require cleanup."
        ));
    }
    execution
}

fn skipped_update_available_components(
    component_decisions: &[ComponentReconcileDecision],
    reason: &str,
) -> Vec<AutoSafeComponentResult> {
    component_decisions
        .iter()
        .filter(|decision| decision.status == ReconcileDecisionStatus::UpdateAvailable)
        .map(|decision| AutoSafeComponentResult {
            id: decision.id.clone(),
            path: PathBuf::new(),
            status: AutoSafeComponentStatus::Skipped,
            reason: reason.to_string(),
        })
        .collect::<Vec<_>>()
}

async fn acquire_auto_safe_update_lock(
    policy: AutoSafeLockPolicy,
) -> std::result::Result<AutoSafeLockAcquireResult, String> {
    let lock_path = auto_safe_update_lock_path()?;
    acquire_auto_safe_update_lock_with_path(lock_path, policy).await
}

async fn acquire_auto_safe_update_lock_with_path(
    lock_path: PathBuf,
    policy: AutoSafeLockPolicy,
) -> std::result::Result<AutoSafeLockAcquireResult, String> {
    if let Some(parent_dir) = lock_path.parent() {
        fs::create_dir_all(parent_dir).map_err(|error| {
            format!(
                "failed to create auto-safe lock directory {}: {error}",
                parent_dir.display()
            )
        })?;
    }

    let mut warnings = Vec::new();
    let started_at = Instant::now();
    loop {
        match try_create_auto_safe_update_lock(&lock_path) {
            Ok(()) => {
                return Ok(AutoSafeLockAcquireResult {
                    guard: AutoSafeLockGuard {
                        path: lock_path,
                        released: false,
                    },
                    warnings,
                });
            }
            Err(error) if is_already_exists_error(&error) => {
                if let Some(stale_reason) =
                    maybe_recover_stale_auto_safe_lock(&lock_path, policy.stale_after)?
                {
                    warnings.push(format!(
                        "Recovered stale auto-safe lock at {} ({stale_reason}).",
                        lock_path.display()
                    ));
                    continue;
                }
                if started_at.elapsed() >= policy.timeout {
                    return Err(format!(
                        "auto-safe lock acquisition timed out after {}ms (retry {}ms) at {}",
                        policy.timeout.as_millis(),
                        policy.retry_interval.as_millis(),
                        lock_path.display()
                    ));
                }
                sleep(policy.retry_interval).await;
            }
            Err(error) => {
                return Err(format!(
                    "failed to acquire auto-safe lock {}: {error}",
                    lock_path.display()
                ));
            }
        }
    }
}

fn try_create_auto_safe_update_lock(lock_path: &Path) -> std::io::Result<()> {
    let mut lock_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(lock_path)?;
    let payload = serde_json::to_vec(&AutoSafeLockMetadata {
        pid: std::process::id(),
        acquired_at_epoch_secs: now_epoch_secs(),
    })
    .map_err(std::io::Error::other)?;
    lock_file.write_all(&payload)?;
    lock_file.flush()?;
    Ok(())
}

fn maybe_recover_stale_auto_safe_lock(
    lock_path: &Path,
    stale_after: Duration,
) -> std::result::Result<Option<String>, String> {
    let Some(reason) = stale_lock_reason(lock_path, stale_after)? else {
        return Ok(None);
    };

    fs::remove_file(lock_path).map_err(|error| {
        format!(
            "failed to remove stale auto-safe lock {}: {error}",
            lock_path.display()
        )
    })?;
    Ok(Some(reason))
}

fn stale_lock_reason(
    lock_path: &Path,
    stale_after: Duration,
) -> std::result::Result<Option<String>, String> {
    let payload = fs::read(lock_path).map_err(|error| {
        format!(
            "failed to read auto-safe lock {}: {error}",
            lock_path.display()
        )
    })?;

    match serde_json::from_slice::<AutoSafeLockMetadata>(&payload) {
        Ok(metadata) => {
            let age_secs = now_epoch_secs().saturating_sub(metadata.acquired_at_epoch_secs);
            if age_secs > stale_after.as_secs() {
                Ok(Some(format!(
                    "age={}s exceeds stale threshold {}s (pid={})",
                    age_secs,
                    stale_after.as_secs(),
                    metadata.pid
                )))
            } else {
                Ok(None)
            }
        }
        Err(_) => {
            let metadata = fs::metadata(lock_path).map_err(|error| {
                format!(
                    "failed to inspect auto-safe lock metadata {}: {error}",
                    lock_path.display()
                )
            })?;
            let modified_at = metadata.modified().map_err(|error| {
                format!(
                    "failed to read auto-safe lock modified timestamp {}: {error}",
                    lock_path.display()
                )
            })?;
            let age_secs = age_from_system_time_secs(modified_at);
            if age_secs > stale_after.as_secs() {
                Ok(Some(format!(
                    "invalid lock metadata with age={}s exceeds stale threshold {}s",
                    age_secs,
                    stale_after.as_secs()
                )))
            } else {
                Ok(None)
            }
        }
    }
}

fn auto_safe_update_lock_path() -> std::result::Result<PathBuf, String> {
    if let Ok(raw_path) = std::env::var(AUTO_SAFE_UPDATE_LOCK_PATH_ENV_VAR) {
        let trimmed = raw_path.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    let cache_root =
        get_cache_dir().map_err(|error| format!("failed to resolve cache directory: {error}"))?;
    Ok(cache_root.join(AUTO_SAFE_UPDATE_LOCK_RELATIVE_PATH))
}

fn release_auto_safe_update_lock(lock_path: &Path) -> std::result::Result<(), String> {
    match fs::remove_file(lock_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "failed to remove auto-safe lock {}: {error}",
            lock_path.display()
        )),
    }
}

fn is_already_exists_error(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::AlreadyExists
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn age_from_system_time_secs(system_time: SystemTime) -> u64 {
    SystemTime::now()
        .duration_since(system_time)
        .unwrap_or_default()
        .as_secs()
}

fn parse_u64_env_or_default(env_var: &str, default_value: u64) -> u64 {
    match std::env::var(env_var) {
        Ok(value) => value.trim().parse::<u64>().unwrap_or(default_value),
        Err(_) => default_value,
    }
}

pub(in crate::commands::agent) fn apply_staged_component_updates(
    staged: Vec<StagedComponentUpdate>,
) -> AutoSafeApplyResult {
    let attempted = staged.len();
    let mut backups = Vec::<ComponentBackup>::new();
    let mut results = Vec::<AutoSafeComponentResult>::new();
    let mut rollback_reason = None;

    for update in staged {
        let mut component_failed = false;
        for write in &update.writes {
            let backup = match fs::read(&write.path) {
                Ok(existing) => ComponentBackup {
                    id: update.id.clone(),
                    path: write.path.clone(),
                    original_bytes: Some(existing),
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => ComponentBackup {
                    id: update.id.clone(),
                    path: write.path.clone(),
                    original_bytes: None,
                },
                Err(error) => {
                    let reason = format!("failed_to_read_existing_file({error})");
                    results.push(AutoSafeComponentResult {
                        id: update.id.clone(),
                        path: update.display_path.clone(),
                        status: AutoSafeComponentStatus::Failed,
                        reason: reason.clone(),
                    });
                    rollback_reason = Some(reason.clone());
                    rollback_component_writes(&mut results, &backups, &reason);
                    component_failed = true;
                    break;
                }
            };

            match write_component_bytes(&write.path, &write.bytes) {
                Ok(()) => backups.push(backup),
                Err(error) => {
                    let failure_reason = format!("failed_to_write_component({error})");
                    results.push(AutoSafeComponentResult {
                        id: update.id.clone(),
                        path: update.display_path.clone(),
                        status: AutoSafeComponentStatus::Failed,
                        reason: failure_reason.clone(),
                    });
                    rollback_reason = Some(failure_reason.clone());
                    rollback_component_writes(&mut results, &backups, &failure_reason);
                    component_failed = true;
                    break;
                }
            }
        }

        if component_failed {
            break;
        }

        results.push(AutoSafeComponentResult {
            id: update.id,
            path: update.display_path,
            status: AutoSafeComponentStatus::Applied,
            reason: "applied_remote_component_update".to_string(),
        });
    }

    summarize_auto_safe_apply_result(
        attempted,
        rollback_reason.is_some(),
        rollback_reason,
        results,
    )
}

pub(in crate::commands::agent) fn rollback_component_writes(
    results: &mut [AutoSafeComponentResult],
    backups: &[ComponentBackup],
    failure_reason: &str,
) {
    let mut failed_rollbacks = HashSet::<String>::new();
    for backup in backups.iter().rev() {
        let restore_result = match backup.original_bytes.as_ref() {
            Some(bytes) => write_component_bytes(&backup.path, bytes),
            None => match fs::remove_file(&backup.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
        };

        if let Err(error) = restore_result {
            failed_rollbacks.insert(backup.id.clone());
            if let Some(existing_result) = results.iter_mut().find(|entry| {
                entry.id == backup.id && entry.status == AutoSafeComponentStatus::Applied
            }) {
                existing_result.status = AutoSafeComponentStatus::Failed;
                existing_result.reason =
                    format!("rollback_failed_after_failure({failure_reason}): {error}");
            }
        }
    }

    for existing_result in results
        .iter_mut()
        .filter(|entry| entry.status == AutoSafeComponentStatus::Applied)
    {
        if failed_rollbacks.contains(&existing_result.id) {
            continue;
        }
        existing_result.status = AutoSafeComponentStatus::RolledBack;
        existing_result.reason = format!("rolled_back_after_failure({failure_reason})");
    }
}

pub(in crate::commands::agent) fn summarize_auto_safe_apply_result(
    attempted: usize,
    rolled_back: bool,
    rollback_reason: Option<String>,
    components: Vec<AutoSafeComponentResult>,
) -> AutoSafeApplyResult {
    let mut applied = 0;
    let mut skipped = 0;
    let mut failed = 0;
    for component in &components {
        match component.status {
            AutoSafeComponentStatus::Applied => applied += 1,
            AutoSafeComponentStatus::Skipped => skipped += 1,
            AutoSafeComponentStatus::Failed | AutoSafeComponentStatus::RolledBack => failed += 1,
        }
    }

    AutoSafeApplyResult {
        attempted,
        applied,
        skipped,
        failed,
        rolled_back,
        rollback_reason,
        components,
    }
}

pub(in crate::commands::agent) fn staged_component_update_from_payload(
    local_component: &ManagedComponentSnapshot,
    remote_component: &ManagedUpdateManifestComponent,
    bytes: Vec<u8>,
) -> std::result::Result<StagedComponentUpdate, String> {
    match local_component.kind {
        ManagedComponentKind::Skill => {
            let skill_root = skill_root_from_snapshot(local_component);
            let writes = if is_archive_source_url(&remote_component.source_url) {
                extract_skill_archive_writes(&skill_root, &bytes, &remote_component.source_url)?
            } else {
                vec![StagedFileWrite {
                    path: local_component.path.clone(),
                    bytes,
                }]
            };

            if writes.is_empty() {
                return Err("empty_skill_payload".to_string());
            }

            Ok(StagedComponentUpdate {
                id: local_component.id.clone(),
                display_path: skill_root,
                writes,
            })
        }
        ManagedComponentKind::McpConfig | ManagedComponentKind::DiscoveryGuide => {
            if is_archive_source_url(&remote_component.source_url) {
                return Err("archive_payload_not_supported_for_file_component".to_string());
            }
            Ok(StagedComponentUpdate {
                id: local_component.id.clone(),
                display_path: local_component.path.clone(),
                writes: vec![StagedFileWrite {
                    path: local_component.path.clone(),
                    bytes,
                }],
            })
        }
    }
}

pub(in crate::commands::agent) fn skill_root_from_snapshot(
    component: &ManagedComponentSnapshot,
) -> PathBuf {
    if component
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("SKILL.md"))
    {
        component
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| component.path.clone())
    } else {
        component.path.clone()
    }
}

pub(in crate::commands::agent) fn extract_skill_archive_writes(
    skill_root: &Path,
    archive_bytes: &[u8],
    source_url: &str,
) -> std::result::Result<Vec<StagedFileWrite>, String> {
    let normalized_source = source_url.trim().to_ascii_lowercase();
    let entries = if normalized_source.ends_with(".tar.gz") || normalized_source.ends_with(".tgz") {
        let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(archive_bytes));
        extract_tar_file_entries(decoder)?
    } else if normalized_source.ends_with(".tar") {
        extract_tar_file_entries(std::io::Cursor::new(archive_bytes))?
    } else {
        return Err(format!(
            "unsupported_skill_archive_format(source_url={source_url})"
        ));
    };

    let normalized_entries = normalize_skill_archive_entries(entries)?;
    if !normalized_entries
        .iter()
        .any(|(path, _)| path == Path::new("SKILL.md"))
    {
        return Err("skill_archive_missing_SKILL.md".to_string());
    }

    let mut seen_paths = HashSet::<PathBuf>::new();
    let mut writes = Vec::with_capacity(normalized_entries.len());
    for (relative_path, file_bytes) in normalized_entries {
        let absolute_path = skill_root.join(&relative_path);
        if !seen_paths.insert(absolute_path.clone()) {
            return Err(format!(
                "skill_archive_contains_duplicate_file({})",
                relative_path.display()
            ));
        }
        writes.push(StagedFileWrite {
            path: absolute_path,
            bytes: file_bytes,
        });
    }

    Ok(writes)
}

pub(in crate::commands::agent) fn extract_tar_file_entries<R: Read>(
    reader: R,
) -> std::result::Result<Vec<(PathBuf, Vec<u8>)>, String> {
    let mut archive = tar::Archive::new(reader);
    let mut files = Vec::<(PathBuf, Vec<u8>)>::new();

    for entry_result in archive
        .entries()
        .map_err(|error| format!("failed_to_read_archive_entries({error})"))?
    {
        let mut entry =
            entry_result.map_err(|error| format!("failed_to_read_archive_entry({error})"))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }

        let raw_path = entry
            .path()
            .map_err(|error| format!("failed_to_read_archive_path({error})"))?;
        let relative_path = sanitize_archive_relative_path(raw_path.as_ref())?;
        let mut file_bytes = Vec::new();
        entry
            .read_to_end(&mut file_bytes)
            .map_err(|error| format!("failed_to_read_archive_file({error})"))?;

        files.push((relative_path, file_bytes));
    }

    if files.is_empty() {
        return Err("archive_contains_no_files".to_string());
    }

    Ok(files)
}

pub(in crate::commands::agent) fn sanitize_archive_relative_path(
    path: &Path,
) -> std::result::Result<PathBuf, String> {
    let mut sanitized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(segment) => sanitized.push(segment),
            std::path::Component::CurDir => {}
            _ => {
                return Err(format!("archive_contains_unsafe_path({})", path.display()));
            }
        }
    }

    if sanitized.as_os_str().is_empty() {
        return Err("archive_contains_empty_path".to_string());
    }

    Ok(sanitized)
}

pub(in crate::commands::agent) fn normalize_skill_archive_entries(
    mut entries: Vec<(PathBuf, Vec<u8>)>,
) -> std::result::Result<Vec<(PathBuf, Vec<u8>)>, String> {
    if entries.is_empty() {
        return Ok(entries);
    }

    let root_component = entries
        .first()
        .and_then(|(path, _)| path.components().next())
        .and_then(|component| match component {
            std::path::Component::Normal(segment) => Some(segment.to_os_string()),
            _ => None,
        });

    let should_strip_root = root_component.as_ref().is_some_and(|root| {
        entries.iter().all(|(path, _)| {
            let mut components = path.components();
            matches!(components.next(), Some(std::path::Component::Normal(segment)) if segment == root.as_os_str())
                && components.next().is_some()
        })
    });

    if should_strip_root {
        for (path, _) in &mut entries {
            let mut stripped = PathBuf::new();
            let mut components = path.components();
            let _ = components.next();
            for component in components {
                if let std::path::Component::Normal(segment) = component {
                    stripped.push(segment);
                }
            }
            if stripped.as_os_str().is_empty() {
                return Err("archive_contains_empty_path_after_root_strip".to_string());
            }
            *path = stripped;
        }
    }

    Ok(entries)
}

pub(in crate::commands::agent) fn write_component_bytes(
    path: &Path,
    bytes: &[u8],
) -> std::io::Result<()> {
    if let Some(parent_dir) = path.parent() {
        fs::create_dir_all(parent_dir)?;
    }
    fs::write(path, bytes)
}

pub(in crate::commands::agent) fn is_auto_safe_apply_supported_kind(
    kind: ManagedComponentKind,
) -> bool {
    matches!(
        kind,
        ManagedComponentKind::Skill
            | ManagedComponentKind::McpConfig
            | ManagedComponentKind::DiscoveryGuide
    )
}

pub(in crate::commands::agent) fn is_archive_source_url(source_url: &str) -> bool {
    let normalized = source_url.trim().to_ascii_lowercase();
    normalized.ends_with(".tar.gz")
        || normalized.ends_with(".tgz")
        || normalized.ends_with(".tar")
        || normalized.ends_with(".zip")
}

pub(in crate::commands::agent) async fn fetch_remote_component_bytes(
    client: &reqwest::Client,
    remote_source: &str,
    component: &ManagedUpdateManifestComponent,
) -> std::result::Result<Vec<u8>, String> {
    let parsed_url = url::Url::parse(component.source_url.trim())
        .map_err(|error| format!("invalid source_url: {error}"))?;
    let request = maybe_attach_registry_auth(client.get(parsed_url), remote_source);
    let response = request
        .send()
        .await
        .map_err(|error| format!("request failed: {error}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {body}"));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("failed to read response body: {error}"))?;
    let actual_checksum = sha256_hex(bytes.as_ref());
    if !actual_checksum.eq_ignore_ascii_case(component.checksum_sha256.trim()) {
        return Err(format!(
            "checksum_mismatch(expected={},actual={actual_checksum})",
            component.checksum_sha256.trim()
        ));
    }

    Ok(bytes.to_vec())
}

pub(in crate::commands::agent) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn auto_safe_lock_times_out_with_deterministic_retry_policy() {
        let temp_dir = tempfile::tempdir().expect("expected temp dir");
        let lock_path = temp_dir.path().join("auto-safe.lock");
        try_create_auto_safe_update_lock(&lock_path).expect("expected initial lock create");

        let error = acquire_auto_safe_update_lock_with_path(
            lock_path.clone(),
            AutoSafeLockPolicy {
                timeout: Duration::from_millis(40),
                retry_interval: Duration::from_millis(10),
                stale_after: Duration::from_secs(600),
            },
        )
        .await
        .expect_err("expected lock timeout");

        assert!(error.contains("timed out"));
        assert!(error.contains("40ms"));
        assert!(error.contains("10ms"));

        release_auto_safe_update_lock(&lock_path).expect("expected lock cleanup");
    }

    #[tokio::test]
    async fn auto_safe_lock_recovers_stale_lock_and_reports_warning() {
        let temp_dir = tempfile::tempdir().expect("expected temp dir");
        let lock_path = temp_dir.path().join("auto-safe.lock");
        let stale_metadata = AutoSafeLockMetadata {
            pid: 4242,
            acquired_at_epoch_secs: now_epoch_secs().saturating_sub(120),
        };
        fs::write(
            &lock_path,
            serde_json::to_vec(&stale_metadata).expect("expected metadata serialization"),
        )
        .expect("expected stale lock seed");

        let acquired = acquire_auto_safe_update_lock_with_path(
            lock_path.clone(),
            AutoSafeLockPolicy {
                timeout: Duration::from_millis(50),
                retry_interval: Duration::from_millis(10),
                stale_after: Duration::from_secs(1),
            },
        )
        .await
        .expect("expected stale lock recovery");

        assert_eq!(acquired.warnings.len(), 1);
        assert!(acquired.warnings[0].contains("Recovered stale auto-safe lock"));
        acquired
            .guard
            .release()
            .expect("expected lock release after recovery");
    }

    #[test]
    fn stale_lock_reason_uses_metadata_age_threshold() {
        let temp_dir = tempfile::tempdir().expect("expected temp dir");
        let lock_path = temp_dir.path().join("auto-safe.lock");
        let fresh_metadata = AutoSafeLockMetadata {
            pid: 1,
            acquired_at_epoch_secs: now_epoch_secs(),
        };
        fs::write(
            &lock_path,
            serde_json::to_vec(&fresh_metadata).expect("expected metadata serialization"),
        )
        .expect("expected lock seed");

        let fresh =
            stale_lock_reason(&lock_path, Duration::from_secs(5)).expect("expected stale check");
        assert!(fresh.is_none());

        let stale_metadata = AutoSafeLockMetadata {
            pid: 1,
            acquired_at_epoch_secs: now_epoch_secs().saturating_sub(10),
        };
        fs::write(
            &lock_path,
            serde_json::to_vec(&stale_metadata).expect("expected metadata serialization"),
        )
        .expect("expected stale lock seed");

        let stale = stale_lock_reason(&lock_path, Duration::from_secs(1))
            .expect("expected stale check")
            .expect("expected stale reason");
        assert!(stale.contains("exceeds stale threshold"));
    }
}
