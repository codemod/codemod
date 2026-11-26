//! Whether the CLI may ask the person running it a question.

use std::io::IsTerminal;

/// Prompts need a terminal on both ends and no CI: without them the run is
/// unattended, whatever the flags say.
pub(crate) fn is_interactive(no_interactive: bool) -> bool {
    interactive(
        no_interactive,
        std::io::stdin().is_terminal() && std::io::stderr().is_terminal(),
        std::env::var("CI").ok().as_deref(),
    )
}

/// Whether the `CI` environment variable says this is a CI job: set to
/// anything but empty, `0` or `false` (`CI=true`, `CI=1`, ...).
pub(crate) fn in_ci(ci: Option<&str>) -> bool {
    ci.map(str::trim)
        .is_some_and(|ci| !ci.is_empty() && ci != "0" && !ci.eq_ignore_ascii_case("false"))
}

fn interactive(no_interactive: bool, terminal: bool, ci: Option<&str>) -> bool {
    !no_interactive && terminal && !in_ci(ci)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_and_no_terminal_are_unattended() {
        assert!(interactive(false, true, None));
        assert!(interactive(false, true, Some("false")));
        assert!(interactive(false, true, Some("0")));
        assert!(interactive(false, true, Some("")));
        assert!(!interactive(false, true, Some("true")));
        assert!(!interactive(false, true, Some("1")));
        assert!(!interactive(false, false, None));
        assert!(!interactive(true, true, None));
    }
}
