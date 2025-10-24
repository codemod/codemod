use codemod_llrt_capabilities::types::LlrtSupportedModules;
use std::{fs, path::PathBuf};

use crate::utils::{ancestor_search::find_in_ancestors, manifest::CodemodManifest};

pub(crate) struct ResolveCapabilitiesArgs {
    pub allow_fs: bool,
    pub allow_fetch: bool,
    pub allow_child_process: bool,
}

/// Loads a manifest from the working directory by searching for codemod.yaml in ancestors
fn load_manifest_from_working_dir(working_directory: &PathBuf) -> Option<CodemodManifest> {
    let manifest_path = find_in_ancestors(working_directory, "codemod.yaml")?;
    let manifest_content = fs::read_to_string(manifest_path).ok()?;
    serde_yaml::from_str(&manifest_content).ok()
}

/// Extracts and parses capabilities from a manifest
fn extract_capabilities(manifest: CodemodManifest) -> Vec<LlrtSupportedModules> {
    manifest
        .capabilities
        .unwrap_or_default()
        .into_iter()
        .filter_map(|s| s.parse::<LlrtSupportedModules>().ok())
        .collect()
}

pub(crate) fn resolve_capabilities(
    args: ResolveCapabilitiesArgs,
    manifest: Option<CodemodManifest>,
    working_directory: Option<PathBuf>,
) -> Vec<LlrtSupportedModules> {
    let mut capabilities = Vec::new();

    // Load capabilities from codemod.yaml in working directory ancestors
    if let Some(working_directory) = working_directory {
        if let Some(manifest) = load_manifest_from_working_dir(&working_directory) {
            capabilities.extend(extract_capabilities(manifest));
        }
    }

    // Load capabilities from provided manifest
    if let Some(manifest) = manifest {
        capabilities.extend(extract_capabilities(manifest));
    }

    // Add capabilities from CLI args
    if args.allow_fs {
        capabilities.push(LlrtSupportedModules::Fs);
    }
    if args.allow_fetch {
        capabilities.push(LlrtSupportedModules::Fetch);
    }
    if args.allow_child_process {
        capabilities.push(LlrtSupportedModules::ChildProcess);
    }

    capabilities
}
