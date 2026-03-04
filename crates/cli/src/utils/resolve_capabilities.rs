use codemod_llrt_capabilities::module_builder::UNSAFE_MODULES;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use inquire::Confirm;
use std::{collections::HashSet, fs, path::PathBuf};

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
fn extract_capabilities(manifest: CodemodManifest) -> HashSet<LlrtSupportedModules> {
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
) -> HashSet<LlrtSupportedModules> {
    let mut capabilities = HashSet::new();

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
        capabilities.insert(LlrtSupportedModules::Fs);
    }
    if args.allow_fetch {
        capabilities.insert(LlrtSupportedModules::Fetch);
    }
    if args.allow_child_process {
        capabilities.insert(LlrtSupportedModules::ChildProcess);
    }

    capabilities
}

/// Prompt the user to approve unsafe capabilities that were resolved from the manifest.
/// Returns the filtered set (safe modules pass through, unsafe ones require approval).
/// Capabilities already granted via CLI flags (`cli_granted`) are not prompted for.
/// If `no_interactive` is true, all capabilities pass through without prompting.
pub(crate) fn prompt_capabilities(
    capabilities: HashSet<LlrtSupportedModules>,
    cli_granted: &HashSet<LlrtSupportedModules>,
    no_interactive: bool,
) -> HashSet<LlrtSupportedModules> {
    if no_interactive {
        // In non-interactive mode, strip unsafe capabilities that were not
        // explicitly granted via CLI flags to avoid implicitly granting
        // dangerous permissions in CI/headless environments.
        let unsafe_set: HashSet<LlrtSupportedModules> = UNSAFE_MODULES.iter().copied().collect();
        return capabilities
            .into_iter()
            .filter(|c| !unsafe_set.contains(c) || cli_granted.contains(c))
            .collect();
    }

    let unsafe_set: HashSet<LlrtSupportedModules> = UNSAFE_MODULES.iter().copied().collect();
    let unsafe_requested: Vec<LlrtSupportedModules> = capabilities
        .iter()
        .filter(|c| unsafe_set.contains(c) && !cli_granted.contains(c))
        .copied()
        .collect();

    if unsafe_requested.is_empty() {
        return capabilities;
    }

    println!();
    println!("  This codemod requests the following permissions:");
    println!();
    for cap in &unsafe_requested {
        let desc = match cap {
            LlrtSupportedModules::Fs => "File system access (read/write files)",
            LlrtSupportedModules::Fetch => "Network access (HTTP requests)",
            LlrtSupportedModules::ChildProcess => "Run shell commands (child processes)",
            _ => "Unknown capability",
        };
        println!("   - {:?} -- {}", cap, desc);
    }
    println!();

    let answer = Confirm::new("Grant these permissions?")
        .with_default(true)
        .with_help_message("Deny to run without these capabilities (codemod may fail)")
        .prompt()
        .unwrap_or(false);

    if answer {
        capabilities
    } else {
        // Strip the denied unsafe capabilities, keep safe ones + CLI-granted ones
        capabilities
            .into_iter()
            .filter(|c| !unsafe_set.contains(c) || cli_granted.contains(c))
            .collect()
    }
}
