//! Whether the CLI may ask the person running it a question.

use std::io::IsTerminal;

/// Prompts need a terminal on both ends: without one the run is unattended,
/// whatever the flags say.
pub(crate) fn is_interactive(no_interactive: bool) -> bool {
    !no_interactive && std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}
