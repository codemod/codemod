//! How hints tell someone to run this CLI again.
//!
//! Hints pin the running version (`npx codemod@2.0.0-alpha.7 login`)
//! instead of the `latest` tag: while 2.x is a prerelease, `latest` is 1.x,
//! a different CLI. The MCP launcher `codemod ai` writes is pinned the same
//! way.

use crate::CLI_VERSION;

/// `codemod@<running version>`, the npm package spec of this CLI.
pub(crate) fn codemod_package() -> String {
    format!("codemod@{CLI_VERSION}")
}

/// `npx codemod@<running version> <args>`.
pub(crate) fn npx_codemod(args: &str) -> String {
    format!("npx {} {args}", codemod_package())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_run_this_version_not_latest() {
        assert_eq!(
            npx_codemod("login"),
            format!("npx codemod@{CLI_VERSION} login")
        );
        assert!(!npx_codemod("login").contains("@latest"));
    }

    /// No hint in the CLI's own messages sends someone to `codemod@latest`.
    #[test]
    fn no_message_points_at_codemod_latest() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for entry in walkdir::WalkDir::new(&src) {
            let entry = entry.unwrap();
            if entry.path().extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(entry.path()).unwrap();
            // Only code before the file's tests: test data may name `@latest`.
            let code = text.split("#[cfg(test)]").next().unwrap_or_default();
            for (number, line) in code.lines().enumerate() {
                if line.contains("codemod@latest") {
                    offenders.push(format!("{}:{}", entry.path().display(), number + 1));
                }
            }
        }
        assert!(offenders.is_empty(), "{offenders:#?}");
    }
}
