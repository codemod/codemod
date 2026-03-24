use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub(in crate::commands::ai) const MANAGED_UPDATE_POLICY_TRIGGER: &str = "install_and_periodic";
pub(in crate::commands::ai) const MANAGED_UPDATE_MANIFEST_PUBLIC_KEYS_ENV_VAR: &str =
    "CODEMOD_AI_UPDATE_MANIFEST_PUBLIC_KEYS";
pub(in crate::commands::ai) const MANAGED_UPDATE_POLICY_LOCAL_SOURCE: &str = "local_embedded_only";
pub(in crate::commands::ai) const MANAGED_UPDATE_REGISTRY_MANIFEST_PATH: &str =
    "/api/v1/ai/managed-components/manifest";
pub(in crate::commands::ai) const MANAGED_UPDATE_MANIFEST_SIGNATURES_HEADER: &str =
    "x-codemod-manifest-signatures-ed25519";
pub(in crate::commands::ai) const MANAGED_UPDATE_MANIFEST_REQUEST_TIMEOUT_SECS: u64 = 3;
pub(in crate::commands::ai) const MANAGED_UPDATE_MANIFEST_CACHE_TTL_SECS: u64 = 3600;
pub(in crate::commands::ai) const MANAGED_UPDATE_MANIFEST_CACHE_RELATIVE_DIR: &str =
    "ai/managed-component-manifests";
pub(in crate::commands::ai) const CURRENT_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(in crate::commands::ai) enum UpdatePolicyMode {
    Manual,
    Notify,
    AutoSafe,
}

impl UpdatePolicyMode {
    pub(in crate::commands::ai) fn as_str(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Notify => "notify",
            Self::AutoSafe => "auto-safe",
        }
    }
}

#[derive(Clone, Debug)]
pub(in crate::commands::ai) struct UpdatePolicyContext {
    pub(in crate::commands::ai) mode: UpdatePolicyMode,
    pub(in crate::commands::ai) remote_source: String,
    pub(in crate::commands::ai) fallback_applied: bool,
    pub(in crate::commands::ai) remote_manifest: Option<RemoteManifestSnapshot>,
    pub(in crate::commands::ai) warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub(in crate::commands::ai) struct RemoteManifestSnapshot {
    pub(in crate::commands::ai) source: String,
    pub(in crate::commands::ai) manifest: ManagedUpdateManifest,
    pub(in crate::commands::ai) authenticity_verified: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::commands::ai) enum ReconcileDecisionStatus {
    UpToDate,
    UpdateAvailable,
    Incompatible,
    Unverifiable,
}

impl ReconcileDecisionStatus {
    pub(in crate::commands::ai) fn as_str(&self) -> &'static str {
        match self {
            Self::UpToDate => "up_to_date",
            Self::UpdateAvailable => "update_available",
            Self::Incompatible => "incompatible",
            Self::Unverifiable => "unverifiable",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::commands::ai) struct ComponentReconcileDecision {
    pub(in crate::commands::ai) id: String,
    pub(in crate::commands::ai) kind: String,
    pub(in crate::commands::ai) local_version: Option<String>,
    pub(in crate::commands::ai) remote_version: Option<String>,
    pub(in crate::commands::ai) status: ReconcileDecisionStatus,
    pub(in crate::commands::ai) reason: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::commands::ai) enum AutoSafeComponentStatus {
    Applied,
    Skipped,
    Failed,
    RolledBack,
}

impl AutoSafeComponentStatus {
    pub(in crate::commands::ai) fn as_str(&self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
            Self::RolledBack => "rolled_back",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::commands::ai) struct AutoSafeComponentResult {
    pub(in crate::commands::ai) id: String,
    pub(in crate::commands::ai) path: PathBuf,
    pub(in crate::commands::ai) status: AutoSafeComponentStatus,
    pub(in crate::commands::ai) reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::commands::ai) struct AutoSafeApplyResult {
    pub(in crate::commands::ai) attempted: usize,
    pub(in crate::commands::ai) applied: usize,
    pub(in crate::commands::ai) skipped: usize,
    pub(in crate::commands::ai) failed: usize,
    pub(in crate::commands::ai) rolled_back: bool,
    pub(in crate::commands::ai) rollback_reason: Option<String>,
    pub(in crate::commands::ai) components: Vec<AutoSafeComponentResult>,
}

#[derive(Clone, Debug)]
pub(in crate::commands::ai) struct StagedComponentUpdate {
    pub(in crate::commands::ai) id: String,
    pub(in crate::commands::ai) display_path: PathBuf,
    pub(in crate::commands::ai) writes: Vec<StagedFileWrite>,
}

#[derive(Clone, Debug)]
pub(in crate::commands::ai) struct StagedFileWrite {
    pub(in crate::commands::ai) path: PathBuf,
    pub(in crate::commands::ai) bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(in crate::commands::ai) struct ComponentBackup {
    pub(in crate::commands::ai) id: String,
    pub(in crate::commands::ai) path: PathBuf,
    pub(in crate::commands::ai) original_bytes: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Default)]
pub(in crate::commands::ai) struct AutoSafeApplyExecution {
    pub(in crate::commands::ai) result: Option<AutoSafeApplyResult>,
    pub(in crate::commands::ai) warnings: Vec<String>,
}

#[derive(Clone, Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(in crate::commands::ai) struct ManagedUpdateManifest {
    pub(in crate::commands::ai) schema_version: String,
    #[serde(default)]
    pub(in crate::commands::ai) generated_at: Option<String>,
    pub(in crate::commands::ai) components: Vec<ManagedUpdateManifestComponent>,
}

#[derive(Clone, Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(in crate::commands::ai) struct ManagedUpdateManifestComponent {
    pub(in crate::commands::ai) id: String,
    pub(in crate::commands::ai) kind: String,
    pub(in crate::commands::ai) version: String,
    pub(in crate::commands::ai) checksum_sha256: String,
    pub(in crate::commands::ai) source_url: String,
    #[serde(default)]
    pub(in crate::commands::ai) min_cli_version: Option<String>,
    #[serde(default)]
    pub(in crate::commands::ai) max_cli_version: Option<String>,
    #[serde(default)]
    pub(in crate::commands::ai) harnesses: Option<Vec<String>>,
}
