use codemod_llrt_capabilities::module_builder::UNSAFE_MODULES;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use inquire::Confirm;
use std::{collections::HashSet, fs, path::PathBuf};

use crate::utils::{ancestor_search::find_in_ancestors, manifest::CodemodManifest};
use console::style;

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
    if let Some(working_directory) = working_directory
        && let Some(manifest) = load_manifest_from_working_dir(&working_directory)
    {
        capabilities.extend(extract_capabilities(manifest));
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
    dry_run: bool,
) -> HashSet<LlrtSupportedModules> {
    if no_interactive {
        // In non-interactive mode, strip unsafe capabilities that were not
        // explicitly granted via CLI flags to avoid implicitly granting
        // dangerous permissions in CI/headless environments.
        return filter_unapproved_unsafe_capabilities(capabilities, cli_granted);
    }

    let unsafe_set: HashSet<LlrtSupportedModules> = UNSAFE_MODULES.iter().copied().collect();
    let mut unsafe_requested: Vec<LlrtSupportedModules> = capabilities
        .iter()
        .filter(|c| unsafe_set.contains(c) && !cli_granted.contains(c))
        .copied()
        .collect();
    unsafe_requested.sort_by_key(|capability| capability_name(*capability));

    if unsafe_requested.is_empty() {
        return capabilities;
    }

    eprintln!();
    eprintln!(
        "  {} {}",
        style("⚠").yellow().bold(),
        style("Permission request").yellow().bold(),
    );
    eprintln!(
        "  {}",
        style("This codemod needs access to sensitive runtime capabilities.").dim()
    );
    eprintln!();
    for capability in &unsafe_requested {
        eprintln!(
            "  {} {} {}",
            style("•").yellow(),
            style(format!("{:<16}", capability_name(*capability)))
                .cyan()
                .bold(),
            style(capability_description(*capability)).dim(),
        );
    }
    eprintln!();
    eprintln!(
        "  {}",
        style("This access applies only to the current run.").dim()
    );
    if let Some(warning) = capability_dry_run_warning(dry_run) {
        eprintln!();
        eprintln!("  {}", style(warning).yellow().bold());
    }

    let answer = Confirm::new("Grant permissions?")
        .with_default(true)
        .with_help_message("Choose no to continue without them; the codemod may fail")
        .prompt()
        .unwrap_or(false);

    if answer {
        capabilities
    } else {
        // Strip the denied unsafe capabilities, keep safe ones + CLI-granted ones
        filter_unapproved_unsafe_capabilities(capabilities, cli_granted)
    }
}

fn capability_dry_run_warning(dry_run: bool) -> Option<&'static str> {
    dry_run.then_some(
        "Dry-run warning: These capabilities may not respect dry-run protections and could perform destructive actions.",
    )
}

fn capability_name(capability: LlrtSupportedModules) -> &'static str {
    match capability {
        LlrtSupportedModules::Fs => "fs",
        LlrtSupportedModules::Fetch => "fetch",
        LlrtSupportedModules::ChildProcess => "child_process",
        _ => "unknown",
    }
}

fn capability_description(capability: LlrtSupportedModules) -> &'static str {
    match capability {
        LlrtSupportedModules::Fs => "Read and write files",
        LlrtSupportedModules::Fetch => "Make HTTP requests",
        LlrtSupportedModules::ChildProcess => "Run shell commands and child processes",
        _ => "Sensitive runtime access",
    }
}

fn filter_unapproved_unsafe_capabilities(
    capabilities: HashSet<LlrtSupportedModules>,
    cli_granted: &HashSet<LlrtSupportedModules>,
) -> HashSet<LlrtSupportedModules> {
    capabilities
        .into_iter()
        .filter(|capability| {
            !UNSAFE_MODULES.contains(capability) || cli_granted.contains(capability)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denied_child_process_capability_is_not_enabled() {
        let capabilities = [
            LlrtSupportedModules::ChildProcess,
            LlrtSupportedModules::Assert,
        ]
        .into_iter()
        .collect();

        let filtered = filter_unapproved_unsafe_capabilities(capabilities, &HashSet::new());

        assert!(!filtered.contains(&LlrtSupportedModules::ChildProcess));
        assert!(filtered.contains(&LlrtSupportedModules::Assert));
    }

    #[test]
    fn dry_run_capability_warning_is_only_shown_in_dry_run_mode() {
        assert_eq!(
            capability_dry_run_warning(true),
            Some(
                "Dry-run warning: These capabilities may not respect dry-run protections and could perform destructive actions."
            )
        );
        assert_eq!(capability_dry_run_warning(false), None);
    }
}
