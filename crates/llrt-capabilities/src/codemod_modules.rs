//! The codemod runtime modules and how code names them.
//!
//! Code imports them as `codemod/<name>` (`codemod/ast-grep`), the same
//! specifiers the `codemod` npm package exports for Node.js and Bun, or by the
//! older spelling `codemod:<name>`. Both name the same module, which runtimes
//! register under `codemod/<name>`.

/// The codemod runtime modules, by name.
pub const CODEMOD_MODULES: &[&str] = &["ast-grep", "workflow", "metrics", "runtime", "llm"];

/// The module a specifier names (`"ast-grep"` for both `codemod/ast-grep` and
/// `codemod:ast-grep`), if it names one.
pub fn codemod_module(specifier: &str) -> Option<&'static str> {
    let name = specifier
        .strip_prefix("codemod/")
        .or_else(|| specifier.strip_prefix("codemod:"))?;
    CODEMOD_MODULES
        .iter()
        .copied()
        .find(|module| *module == name)
}

/// The specifier runtimes register a module under: `codemod/<name>`.
pub fn codemod_module_specifier(name: &str) -> String {
    format!("codemod/{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_spellings_name_the_same_module() {
        assert_eq!(codemod_module("codemod/ast-grep"), Some("ast-grep"));
        assert_eq!(codemod_module("codemod:ast-grep"), Some("ast-grep"));
        assert_eq!(codemod_module("codemod/workflow"), Some("workflow"));
        assert_eq!(codemod_module("codemod/testing"), None);
        assert_eq!(codemod_module("codemod/ast-grep/langs/java"), None);
        assert_eq!(codemod_module("codemod"), None);
        assert_eq!(codemod_module_specifier("metrics"), "codemod/metrics");
    }
}
