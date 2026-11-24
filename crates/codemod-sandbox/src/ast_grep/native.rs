use std::error::Error;
use std::path::Path;
use std::str::FromStr;
use std::{fs, panic};

use ast_grep_config::{from_yaml_string, CombinedScan, RuleConfig};
use ast_grep_core::tree_sitter::StrDoc;
use ast_grep_core::AstGrep;

use crate::sandbox::engine::codemod_lang::CodemodLang;
type SupportLang = CodemodLang;

use crate::ast_grep::scanner::scan_content;
use crate::ast_grep::types::{AstGrepError, AstGrepMatch};
use crate::ast_grep::utils::detect_language_from_extension;

pub struct CombinedScanWithRuleConfigs<'a> {
    pub combined_scan: CombinedScan<'a, SupportLang>,
    pub rule_refs: Vec<&'a RuleConfig<SupportLang>>,
}

/// The rules of one language, in a scan of their own.
pub struct LanguageScan<'a> {
    pub language: SupportLang,
    pub combined_scan: CombinedScan<'a, SupportLang>,
}

impl<'a> CombinedScanWithRuleConfigs<'a> {
    /// The rules split by language, in the order the languages first come.
    /// A scan matches nodes by kind, which only means something in the
    /// grammar its rules were written for, so a file is scanned with the
    /// rules of each language it is parsed as.
    pub fn by_language(&self) -> Vec<LanguageScan<'a>> {
        let mut groups: Vec<(SupportLang, Vec<&'a RuleConfig<SupportLang>>)> = Vec::new();
        for rule in &self.rule_refs {
            match groups
                .iter_mut()
                .find(|(language, _)| *language == rule.language)
            {
                Some((_, rules)) => rules.push(rule),
                None => groups.push((rule.language, vec![rule])),
            }
        }
        groups
            .into_iter()
            .map(|(language, rules)| LanguageScan {
                language,
                combined_scan: CombinedScan::new(rules),
            })
            .collect()
    }
}

pub fn with_combined_scan<T>(
    config_file_path: &str,
    f: impl for<'a> FnOnce(&CombinedScanWithRuleConfigs<'a>) -> Result<T, Box<dyn Error>>,
) -> Result<T, Box<dyn Error>> {
    let config_content = fs::read_to_string(config_file_path)?;
    with_combined_scan_of(&config_content, f)
}

/// [`with_combined_scan`] for rules given as text: YAML, with documents
/// separated by `---`. JSON is YAML, so rules built as JSON objects work too.
pub fn with_combined_scan_of<T>(
    config_content: &str,
    f: impl for<'a> FnOnce(&CombinedScanWithRuleConfigs<'a>) -> Result<T, Box<dyn Error>>,
) -> Result<T, Box<dyn Error>> {
    let rule_configs = from_yaml_string(config_content, &Default::default())
        .map_err(|e| AstGrepError::Config(format!("Failed to parse YAML rules: {e:?}")))?;

    let combined_scan = CombinedScan::new(rule_configs.iter().collect());
    let rule_refs: Vec<&RuleConfig<SupportLang>> = rule_configs.iter().collect();

    f(&CombinedScanWithRuleConfigs {
        combined_scan,
        rule_refs,
    })
}

thread_local! {
    /// Set while this thread scans a file: a panic then is reported as that
    /// file's error, not printed.
    static SCANNING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Keep panics from scanning threads off the terminal. The hook is installed
/// once and only quiets threads that are scanning, so steps running at the
/// same time do not fight over it.
fn quiet_scan_panics() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if !SCANNING.with(std::cell::Cell::get) {
                previous(info);
            }
        }));
    });
}

pub fn scan_file_with_combined_scan(
    file_path: &Path,
    combined_scan: &CombinedScan<SupportLang>,
    apply_fixes: bool,
) -> Result<(Vec<AstGrepMatch>, bool, Option<String>), AstGrepError> {
    let content = fs::read_to_string(file_path)?;
    scan_text_with_combined_scan(file_path, &content, combined_scan, apply_fixes)
}

/// [`scan_file_with_combined_scan`] on text already read from `file_path`
/// (its extension picks the language).
pub fn scan_text_with_combined_scan(
    file_path: &Path,
    content: &str,
    combined_scan: &CombinedScan<SupportLang>,
    apply_fixes: bool,
) -> Result<(Vec<AstGrepMatch>, bool, Option<String>), AstGrepError> {
    let scan_result = scan_text(file_path, content, combined_scan, apply_fixes)?;
    let file_modified = scan_result.file_modified;
    let new_content = if file_modified {
        Some(scan_result.new_content)
    } else {
        None
    };

    Ok((scan_result.matches, file_modified, new_content))
}

/// One pass of rules over a file's text, with their fixes applied.
pub struct ScanPass {
    /// Matches found, of every rule, overlapping ones included.
    pub matches: usize,
    /// Fixes applied: of overlapping matches, only the outer one is fixed.
    pub fixes: usize,
    /// The new text, when the fixes changed it.
    pub content: Option<String>,
}

/// Scan text already read from `file_path` (its extension picks the
/// language) and apply the fixes, counting what was found and fixed.
pub fn scan_pass(
    file_path: &Path,
    content: &str,
    combined_scan: &CombinedScan<SupportLang>,
) -> Result<ScanPass, AstGrepError> {
    scan_pass_as(language_of(file_path)?, file_path, content, combined_scan)
}

/// [`scan_pass`] with the text parsed as `language`, whatever the file's
/// extension: a `typescript` rule runs on the `.js` files it selects as
/// TypeScript.
pub fn scan_pass_as(
    language: SupportLang,
    file_path: &Path,
    content: &str,
    combined_scan: &CombinedScan<SupportLang>,
) -> Result<ScanPass, AstGrepError> {
    let scanned = scan_text_as(language, file_path, content, combined_scan, true)?;
    Ok(ScanPass {
        matches: scanned.matches.len(),
        fixes: scanned.fixes_applied,
        content: (scanned.file_modified && scanned.new_content != content)
            .then_some(scanned.new_content),
    })
}

/// The language a file's extension says it is in.
fn language_of(file_path: &Path) -> Result<SupportLang, AstGrepError> {
    let language_str = detect_language_from_extension(
        file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default(),
    )?;
    SupportLang::from_str(language_str)
        .map_err(|_| AstGrepError::Language(format!("Language not supported: {language_str}")))
}

fn scan_text(
    file_path: &Path,
    content: &str,
    combined_scan: &CombinedScan<SupportLang>,
    apply_fixes: bool,
) -> Result<crate::ast_grep::scanner::ScanResult, AstGrepError> {
    scan_text_as(
        language_of(file_path)?,
        file_path,
        content,
        combined_scan,
        apply_fixes,
    )
}

fn scan_text_as(
    language: SupportLang,
    file_path: &Path,
    content: &str,
    combined_scan: &CombinedScan<SupportLang>,
    apply_fixes: bool,
) -> Result<crate::ast_grep::scanner::ScanResult, AstGrepError> {
    // Parsing and matching can panic on input the grammar does not expect:
    // that is this file's failure, not the step's.
    quiet_scan_panics();
    SCANNING.with(|scanning| scanning.set(true));
    let scanned = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let root = AstGrep::doc(StrDoc::new(content, language));
        scan_content(
            &root,
            content,
            file_path.to_string_lossy().to_string(),
            combined_scan,
            apply_fixes,
        )
    }));
    SCANNING.with(|scanning| scanning.set(false));
    scanned.map_err(|_| {
        AstGrepError::Config(format!("ast-grep could not scan '{}'", file_path.display()))
    })?
}

#[cfg(test)]
mod tests {
    use super::{scan_file_with_combined_scan, with_combined_scan};

    /// Fix `source` (a `.ts` file) with the rules in `rules` (YAML).
    fn fix(rules: &str, source: &str) -> Option<String> {
        let dir = tempfile::tempdir().unwrap();
        let rules_path = dir.path().join("rules.yml");
        std::fs::write(&rules_path, rules).unwrap();
        let file = dir.path().join("input.ts");
        std::fs::write(&file, source).unwrap();
        with_combined_scan(rules_path.to_str().unwrap(), |scan| {
            let (_, _, content) = scan_file_with_combined_scan(&file, &scan.combined_scan, true)?;
            Ok(content)
        })
        .unwrap()
    }

    #[test]
    fn a_match_nested_in_another_match_of_the_rule_keeps_the_outer_fix() {
        let rules = "id: log\nlanguage: typescript\nrule:\n  pattern: console.log($A)\nfix: logger.info($A)\n";
        assert_eq!(
            fix(rules, "console.log(console.log(1));\nconsole.log(2);\n").as_deref(),
            Some("logger.info(console.log(1));\nlogger.info(2);\n")
        );
    }

    #[test]
    fn overlapping_matches_of_two_rules_keep_the_outer_fix() {
        let rules = "\
id: undefined-check
language: typescript
rule:
  pattern: $A !== undefined
fix: $A != null
---
id: rename-dispose
language: typescript
rule:
  kind: property_identifier
  regex: ^dispose$
fix: disposeNow
";
        assert_eq!(
            fix(rules, "if (x.dispose !== undefined) { y.dispose(); }\n").as_deref(),
            Some("if (x.dispose != null) { y.disposeNow(); }\n")
        );
    }
}
