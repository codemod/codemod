use std::error::Error;
use std::fs;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

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

pub fn ast_grep_parse_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub fn with_combined_scan<T>(
    config_file_path: &str,
    f: impl for<'a> FnOnce(&CombinedScanWithRuleConfigs<'a>) -> Result<T, Box<dyn Error>>,
) -> Result<T, Box<dyn Error>> {
    let config_content = fs::read_to_string(config_file_path)?;
    let rule_configs = {
        // Native tree-sitter parsers are initialized through process-global state.
        // Keep config/rule parser setup serialized with per-file AST creation.
        let _guard = ast_grep_parse_lock()
            .lock()
            .map_err(|_| AstGrepError::Config("ast-grep parser lock was poisoned".to_string()))?;
        from_yaml_string(&config_content, &Default::default())
            .map_err(|e| AstGrepError::Config(format!("Failed to parse YAML rules: {e:?}")))?
    };
    let combined_scan = {
        let _guard = ast_grep_parse_lock()
            .lock()
            .map_err(|_| AstGrepError::Config("ast-grep parser lock was poisoned".to_string()))?;
        CombinedScan::new(rule_configs.iter().collect())
    };
    let rule_refs: Vec<&RuleConfig<SupportLang>> = rule_configs.iter().collect();

    let result = f(&CombinedScanWithRuleConfigs {
        combined_scan,
        rule_refs,
    })?;

    Ok(result)
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

    let _guard = ast_grep_parse_lock()
        .lock()
        .map_err(|_| AstGrepError::Config("ast-grep parser lock was poisoned".to_string()))?;
    let doc = StrDoc::new(&content, language);
    let root = AstGrep::doc(doc);

    let scan_result = scan_content(
        &root,
        &content,
        file_path.to_string_lossy().to_string(),
        combined_scan,
        apply_fixes,
    )?;

    let file_modified = scan_result.file_modified;
    let new_content = if file_modified {
        Some(scan_result.new_content)
    } else {
        None
    };

    Ok((scan_result.matches, file_modified, new_content))
}
