use codemod_llrt_capabilities::module_builder::UNSAFE_MODULES;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use std::{
    collections::HashSet,
    fs,
    io::{self, BufRead, BufReader, IsTerminal, Write},
    path::PathBuf,
};

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

    reset_terminal_for_prompt();

    eprintln!();
    eprintln!("  This codemod requests the following permissions:");
    eprintln!();
    for cap in &unsafe_requested {
        let desc = match cap {
            LlrtSupportedModules::Fs => "File system access (read/write files)",
            LlrtSupportedModules::Fetch => "Network access (HTTP requests)",
            LlrtSupportedModules::ChildProcess => "Run shell commands (child processes)",
            _ => "Unknown capability",
        };
        eprintln!("   - {:?} -- {}", cap, desc);
    }
    eprintln!();

    let answer = prompt_yes_no(
        "Grant these permissions? (Y/n): ",
        true,
        "Deny to run without these capabilities (codemod may fail)",
    )
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

fn prompt_yes_no(prompt: &str, default_yes: bool, help: &str) -> io::Result<bool> {
    let mut input = open_prompt_reader()?;
    let mut stderr = io::stderr().lock();
    prompt_yes_no_with_io(prompt, default_yes, help, &mut input, &mut stderr)
}

fn prompt_yes_no_with_io<R: BufRead, W: Write>(
    prompt: &str,
    default_yes: bool,
    help: &str,
    input: &mut R,
    output: &mut W,
) -> io::Result<bool> {
    loop {
        writeln!(output, "  [{help}]")?;
        write!(output, "  {prompt}")?;
        output.flush()?;

        let mut answer = String::new();
        let bytes_read = input.read_line(&mut answer)?;
        if bytes_read == 0 {
            writeln!(output)?;
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "terminal prompt closed before a response was provided",
            ));
        }

        let answer = answer.trim().to_ascii_lowercase();

        let accepted = match answer.as_str() {
            "" => Some(default_yes),
            "y" | "yes" => Some(true),
            "n" | "no" => Some(false),
            _ => None,
        };

        match accepted {
            Some(value) => {
                writeln!(output)?;
                return Ok(value);
            }
            None => {
                writeln!(output, "  Please answer y or n.")?;
                writeln!(output)?;
            }
        }
    }
}

fn open_prompt_reader() -> io::Result<Box<dyn BufRead>> {
    #[cfg(unix)]
    {
        if let Ok(tty) = fs::OpenOptions::new().read(true).open("/dev/tty") {
            return Ok(Box::new(BufReader::new(tty)));
        }
    }

    #[cfg(windows)]
    {
        if let Ok(tty) = fs::OpenOptions::new().read(true).open("CONIN$") {
            return Ok(Box::new(BufReader::new(tty)));
        }
    }

    let stdin = io::stdin();
    if stdin.is_terminal() {
        return Ok(Box::new(BufReader::new(stdin)));
    }

    Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "interactive prompt is unavailable because stdin is not a terminal",
    ))
}

fn reset_terminal_for_prompt() {
    #[cfg(unix)]
    {
        use crossterm::event::{DisableBracketedPaste, DisableFocusChange, DisableMouseCapture};
        use crossterm::execute;
        use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};

        let _ = disable_raw_mode();
        if let Ok(mut tty) = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
        {
            let _ = execute!(
                tty,
                DisableFocusChange,
                DisableBracketedPaste,
                DisableMouseCapture,
                LeaveAlternateScreen
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_yes_no_accepts_explicit_default_line() {
        let mut input = io::Cursor::new(b"\n");
        let mut output = Vec::new();

        let accepted = prompt_yes_no_with_io(
            "Grant these permissions? (Y/n): ",
            true,
            "help",
            &mut input,
            &mut output,
        )
        .unwrap();

        assert!(accepted);
        let rendered = String::from_utf8(output).unwrap();
        assert!(rendered.contains("Grant these permissions?"));
    }

    #[test]
    fn prompt_yes_no_rejects_eof_without_approving() {
        let mut input = io::Cursor::new(Vec::<u8>::new());
        let mut output = Vec::new();

        let error = prompt_yes_no_with_io(
            "Grant these permissions? (Y/n): ",
            true,
            "help",
            &mut input,
            &mut output,
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }
}
