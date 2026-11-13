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

pub fn with_combined_scan<T>(
    config_file_path: &str,
    f: impl for<'a> FnOnce(&CombinedScanWithRuleConfigs<'a>) -> Result<T, Box<dyn Error>>,
) -> Result<T, Box<dyn Error>> {
    let config_content = fs::read_to_string(config_file_path)?;
    let rule_configs = from_yaml_string(&config_content, &Default::default())
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

    let language_str = detect_language_from_extension(
        file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default(),
    )?;

    let language = SupportLang::from_str(language_str)
        .map_err(|_| AstGrepError::Language(format!("Language not supported: {language_str}")))?;

    // Parsing and matching can panic on input the grammar does not expect:
    // that is this file's failure, not the step's.
    quiet_scan_panics();
    SCANNING.with(|scanning| scanning.set(true));
    let scanned = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let root = AstGrep::doc(StrDoc::new(&content, language));
        scan_content(
            &root,
            &content,
            file_path.to_string_lossy().to_string(),
            combined_scan,
            apply_fixes,
        )
    }));
    SCANNING.with(|scanning| scanning.set(false));
    let scan_result = scanned.map_err(|_| {
        AstGrepError::Config(format!("ast-grep could not scan '{}'", file_path.display()))
    })??;

    let file_modified = scan_result.file_modified;
    let new_content = if file_modified {
        Some(scan_result.new_content)
    } else {
        None
    };

    Ok((scan_result.matches, file_modified, new_content))
}
