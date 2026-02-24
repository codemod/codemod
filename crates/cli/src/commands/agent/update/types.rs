use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub(in crate::commands::agent) const MANAGED_UPDATE_POLICY_TRIGGER: &str = "agent_install";
pub(in crate::commands::agent) const MANAGED_UPDATE_MANIFEST_PUBLIC_KEY_ENV_VAR: &str =
    "CODEMOD_AGENT_UPDATE_MANIFEST_PUBLIC_KEY";
pub(in crate::commands::agent) const MANAGED_UPDATE_POLICY_LOCAL_SOURCE: &str =
    "local_embedded_only";
pub(in crate::commands::agent) const MANAGED_UPDATE_REGISTRY_MANIFEST_PATH: &str =
    "/api/v1/agent/managed-components/manifest";
pub(in crate::commands::agent) const MANAGED_UPDATE_MANIFEST_SIGNATURE_HEADER: &str =
    "x-codemod-manifest-signature-ed25519";
pub(in crate::commands::agent) const MANAGED_UPDATE_MANIFEST_REQUEST_TIMEOUT_SECS: u64 = 3;
pub(in crate::commands::agent) const MANAGED_UPDATE_MANIFEST_CACHE_TTL_SECS: u64 = 3600;
pub(in crate::commands::agent) const MANAGED_UPDATE_MANIFEST_CACHE_RELATIVE_DIR: &str =
    "agent/managed-component-manifests";
pub(in crate::commands::agent) const CURRENT_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(in crate::commands::agent) enum UpdatePolicyMode {
    Manual,
    Notify,
    AutoSafe,
}

impl UpdatePolicyMode {
    pub(in crate::commands::agent) fn as_str(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Notify => "notify",
            Self::AutoSafe => "auto-safe",
        }
    }
}

#[derive(Clone, Debug)]
pub(in crate::commands::agent) struct UpdatePolicyContext {
    pub(in crate::commands::agent) mode: UpdatePolicyMode,
    pub(in crate::commands::agent) remote_source: String,
    pub(in crate::commands::agent) fallback_applied: bool,
    pub(in crate::commands::agent) remote_manifest: Option<RemoteManifestSnapshot>,
    pub(in crate::commands::agent) warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub(in crate::commands::agent) struct RemoteManifestSnapshot {
    pub(in crate::commands::agent) source: String,
    pub(in crate::commands::agent) manifest: ManagedUpdateManifest,
    pub(in crate::commands::agent) authenticity_verified: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::commands::agent) enum ReconcileDecisionStatus {
    UpToDate,
    UpdateAvailable,
    Incompatible,
    Unverifiable,
}

impl ReconcileDecisionStatus {
    pub(in crate::commands::agent) fn as_str(&self) -> &'static str {
        match self {
            Self::UpToDate => "up_to_date",
            Self::UpdateAvailable => "update_available",
            Self::Incompatible => "incompatible",
            Self::Unverifiable => "unverifiable",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::commands::agent) struct ComponentReconcileDecision {
    pub(in crate::commands::agent) id: String,
    pub(in crate::commands::agent) kind: String,
    pub(in crate::commands::agent) local_version: Option<String>,
    pub(in crate::commands::agent) remote_version: Option<String>,
    pub(in crate::commands::agent) status: ReconcileDecisionStatus,
    pub(in crate::commands::agent) reason: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::commands::agent) enum AutoSafeComponentStatus {
    Applied,
    Skipped,
    Failed,
    RolledBack,
}

impl AutoSafeComponentStatus {
    pub(in crate::commands::agent) fn as_str(&self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
            Self::RolledBack => "rolled_back",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::commands::agent) struct AutoSafeComponentResult {
    pub(in crate::commands::agent) id: String,
    pub(in crate::commands::agent) path: PathBuf,
    pub(in crate::commands::agent) status: AutoSafeComponentStatus,
    pub(in crate::commands::agent) reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::commands::agent) struct AutoSafeApplyResult {
    pub(in crate::commands::agent) attempted: usize,
    pub(in crate::commands::agent) applied: usize,
    pub(in crate::commands::agent) skipped: usize,
    pub(in crate::commands::agent) failed: usize,
    pub(in crate::commands::agent) rolled_back: bool,
    pub(in crate::commands::agent) rollback_reason: Option<String>,
    pub(in crate::commands::agent) components: Vec<AutoSafeComponentResult>,
}

#[derive(Clone, Debug)]
pub(in crate::commands::agent) struct StagedComponentUpdate {
    pub(in crate::commands::agent) id: String,
    pub(in crate::commands::agent) display_path: PathBuf,
    pub(in crate::commands::agent) writes: Vec<StagedFileWrite>,
}

#[derive(Clone, Debug)]
pub(in crate::commands::agent) struct StagedFileWrite {
    pub(in crate::commands::agent) path: PathBuf,
    pub(in crate::commands::agent) bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(in crate::commands::agent) struct ComponentBackup {
    pub(in crate::commands::agent) id: String,
    pub(in crate::commands::agent) path: PathBuf,
    pub(in crate::commands::agent) original_bytes: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Default)]
pub(in crate::commands::agent) struct AutoSafeApplyExecution {
    pub(in crate::commands::agent) result: Option<AutoSafeApplyResult>,
    pub(in crate::commands::agent) warnings: Vec<String>,
}

#[derive(Clone, Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(in crate::commands::agent) struct ManagedUpdateManifest {
    pub(in crate::commands::agent) schema_version: String,
    #[serde(default)]
    pub(in crate::commands::agent) generated_at: Option<String>,
    pub(in crate::commands::agent) components: Vec<ManagedUpdateManifestComponent>,
}

#[derive(Clone, Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(in crate::commands::agent) struct ManagedUpdateManifestComponent {
    pub(in crate::commands::agent) id: String,
    pub(in crate::commands::agent) kind: String,
    pub(in crate::commands::agent) version: String,
    pub(in crate::commands::agent) checksum_sha256: String,
    pub(in crate::commands::agent) source_url: String,
    #[serde(default)]
    pub(in crate::commands::agent) min_cli_version: Option<String>,
    #[serde(default)]
    pub(in crate::commands::agent) max_cli_version: Option<String>,
    #[serde(default)]
    pub(in crate::commands::agent) harnesses: Option<Vec<String>>,
}
