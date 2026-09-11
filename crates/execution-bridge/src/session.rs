//! One stateful JSSG session: a resolved script, its module resolver and
//! selector, the language, the invocation input, and an optional semantic
//! provider shared by every file the session transforms.
//!
//! The session never reads a repository listing and never writes repository
//! files. It receives file contents from the host, returns each transform's
//! primary edit, secondary edits, rename information, and JSON output as
//! plain data, and validates every path against the canonical target root.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use codemod_sandbox::{
    sandbox::{
        engine::{
            codemod_lang::CodemodLang, execute_codemod_with_quickjs, extract_selector_with_quickjs,
            language_data::get_extensions_for_language, CodemodOutput, ExecutionResult,
            JssgExecutionOptions, SelectorEngineOptions,
        },
        resolvers::OxcResolver,
    },
    utils::project_discovery::find_tsconfig,
};
use language_core::{ProviderMode, SemanticProvider};
use semantic_factory::LazySemanticProvider;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    paths::{canonical_root, normalize_output, resolve_source},
    validate_relative_path, SemanticAnalysis, SemanticMode,
};

/// Everything needed to open a session. Mirrors the worker `open` message.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Safe relative script path, resolved beneath `script_root`.
    pub script: String,
    /// Absolute script root (executor context, never command identity).
    pub script_root: PathBuf,
    pub language: String,
    /// Root every source and output path is validated against.
    pub target_root: PathBuf,
    pub semantic_analysis: Option<SemanticAnalysis>,
    /// Invocation input, exposed to the transform as `options.params.input`.
    pub input: Option<Value>,
}

/// Facts about an opened session that the host needs before it selects
/// files: the language's default extensions and the semantic mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    /// `**/*<ext>` defaults come from this list when a definition has no
    /// `include`, exactly as `CodemodExecutionConfig::build_globs` derives them.
    pub extensions: Vec<String>,
    pub semantic_mode: Option<SemanticMode>,
}

/// One file's result, with every path already root-relative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum FileResult {
    Modified {
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rename_to: Option<String>,
    },
    Unmodified,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecondaryResult {
    pub path: String,
    pub result: FileResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransformResult {
    pub primary: FileResult,
    pub secondary: Vec<SecondaryResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
}

pub struct JssgSession {
    script_path: PathBuf,
    resolver: Arc<OxcResolver>,
    language: CodemodLang,
    selector: Option<Arc<ast_grep_config::RuleConfig<CodemodLang>>>,
    semantic_provider: Option<Arc<dyn SemanticProvider>>,
    params: Option<HashMap<String, Value>>,
    target_root: PathBuf,
    info: SessionInfo,
}

impl JssgSession {
    /// Resolve the script, load its selector, and build the semantic
    /// provider. Nothing here touches repository files.
    pub async fn open(config: SessionConfig) -> Result<Self, String> {
        validate_relative_path(&config.script, "JSSG script")?;
        if !config.script_root.is_absolute() {
            return Err("scriptRoot must be an absolute path".to_string());
        }
        if !config.target_root.is_absolute() {
            return Err("targetRoot must be an absolute path".to_string());
        }
        let target_root = canonical_root(&config.target_root, "target root")?;
        let script_candidate = config.script_root.join(&config.script);
        let script_path = script_candidate.canonicalize().map_err(|error| {
            format!(
                "failed to resolve JSSG script '{}': {error}",
                script_candidate.display()
            )
        })?;
        let language: CodemodLang = config
            .language
            .parse()
            .map_err(|error| format!("invalid JSSG language '{}': {error}", config.language))?;
        let script_dir = script_path.parent().unwrap_or(std::path::Path::new("."));
        let resolver = Arc::new(
            OxcResolver::new(script_dir.to_path_buf(), find_tsconfig(script_dir))
                .map_err(|error| format!("failed to create JSSG resolver: {error}"))?,
        );
        // The shipped selector engine passes no params, so `getSelector` sees `{}`.
        let selector = extract_selector_with_quickjs(SelectorEngineOptions {
            script_path: &script_path,
            language,
            resolver: Arc::clone(&resolver),
            capabilities: None,
            target_directory: Some(&target_root),
        })
        .await
        .map_err(|error| format!("failed to load JSSG selector: {error}"))?
        .map(Arc::from);
        let semantic_provider =
            build_semantic_provider(config.semantic_analysis.as_ref(), &target_root)?;
        let info = SessionInfo {
            extensions: get_extensions_for_language(language)
                .into_iter()
                .map(str::to_string)
                .collect(),
            semantic_mode: semantic_provider
                .as_ref()
                .map(|provider| match provider.mode() {
                    ProviderMode::FileScope => SemanticMode::File,
                    ProviderMode::WorkspaceScope => SemanticMode::Workspace,
                }),
        };
        Ok(Self {
            script_path,
            resolver,
            language,
            selector,
            semantic_provider,
            params: config
                .input
                .map(|value| HashMap::from([("input".to_string(), value)])),
            target_root,
            info,
        })
    }

    pub fn info(&self) -> &SessionInfo {
        &self.info
    }

    pub fn target_root(&self) -> &std::path::Path {
        &self.target_root
    }

    /// Feed one file into the semantic index. A no-op without a provider;
    /// the host calls this for the workspace set before transforms and for
    /// every staged edit afterwards, matching the engine's post-write refresh.
    pub fn index(&self, path: &str, content: &str) -> Result<(), String> {
        let file = resolve_source(&self.target_root, path, "index path")?;
        if let Some(provider) = &self.semantic_provider {
            provider
                .notify_file_processed(&file, content)
                .map_err(|error| {
                    format!("failed to index '{path}' for semantic analysis: {error}")
                })?;
        }
        Ok(())
    }

    /// Transform one file's content. Every returned path is root-relative and
    /// has been checked against the canonical target root.
    pub async fn transform(&self, path: &str, content: &str) -> Result<TransformResult, String> {
        let file = resolve_source(&self.target_root, path, "transform path")?;
        let output = execute_codemod_with_quickjs(JssgExecutionOptions {
            script_path: &self.script_path,
            resolver: Arc::clone(&self.resolver),
            language: self.language,
            file_path: &file,
            content,
            selector_config: self.selector.clone(),
            params: self.params.clone(),
            matrix_values: None,
            capabilities: None,
            semantic_provider: self.semantic_provider.clone(),
            metrics_context: None,
            llm_request_handler: None,
            shared_state_context: None,
            runtime_event_callback: None,
            cancellation_flag: None,
            test_mode: false,
            dry_run: false,
            stage_writes: true,
            target_directory: &self.target_root,
        })
        .await
        .map_err(|error| format!("JSSG failed for '{path}': {error}"))?;
        self.convert(output)
    }

    fn convert(&self, output: CodemodOutput) -> Result<TransformResult, String> {
        let CodemodOutput {
            primary,
            secondary,
            output,
        } = output;
        let primary = self.convert_result(primary, "rename target")?;
        let secondary = secondary
            .into_iter()
            .map(|change| {
                Ok(SecondaryResult {
                    path: normalize_output(&self.target_root, &change.path, "secondary path")?,
                    result: self.convert_result(change.result, "secondary rename target")?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(TransformResult {
            primary,
            secondary,
            output,
        })
    }

    fn convert_result(&self, result: ExecutionResult, name: &str) -> Result<FileResult, String> {
        Ok(match result {
            ExecutionResult::Modified(modified) => FileResult::Modified {
                content: modified.content,
                rename_to: modified
                    .rename_to
                    .as_deref()
                    .map(|target| normalize_output(&self.target_root, target, name))
                    .transpose()?,
            },
            ExecutionResult::Unmodified => FileResult::Unmodified,
            ExecutionResult::Skipped => FileResult::Skipped,
        })
    }
}

fn build_semantic_provider(
    config: Option<&SemanticAnalysis>,
    target_root: &std::path::Path,
) -> Result<Option<Arc<dyn SemanticProvider>>, String> {
    let Some(config) = config else {
        return Ok(None);
    };
    config.validate()?;
    Ok(Some(match config.mode() {
        SemanticMode::File => Arc::new(LazySemanticProvider::file_scope()),
        SemanticMode::Workspace => {
            let root = match config.root() {
                Some(root) => resolve_source(target_root, root, "semanticAnalysis.root")?,
                None => target_root.to_path_buf(),
            };
            if !root.is_dir() {
                return Err(format!(
                    "failed to resolve semanticAnalysis.root '{}': not a directory",
                    root.display()
                ));
            }
            Arc::new(LazySemanticProvider::workspace_scope(root))
        }
    }))
}
