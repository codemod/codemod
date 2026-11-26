//! The [`SemanticProvider`] for Rust.

use crate::host::RsFlow;
use crate::{modules, symbols};
use language_core::{
    ByteRange, CallGraphOptions, CallGraphResult, DefinitionOptions, DefinitionResult, EntryPoint,
    EntryPointRule, FileSnapshot, FlowOptions, FlowResult, FunctionEntryPoints, ImportersResult,
    MutationOptions, MutationsResult, ProviderMode, ReferencesResult, SemanticError,
    SemanticProvider, SemanticResult, SpecifierOptions,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Semantic analysis for Rust on rust-analyzer's library crates.
///
/// - **Workspace scope** loads the Cargo workspace at the root (with
///   `cargo metadata`, offline), so names, calls and types resolve across
///   files and crates. Without a `Cargo.toml` the `.rs` files are loaded as
///   standalone files.
/// - **File scope** loads each file on its own.
///
/// Build scripts are not run and procedural macros (derives, attribute
/// macros) are not expanded: no compiler process is started.
pub struct RaSemanticProvider {
    flow: RsFlow,
    mode: ProviderMode,
    /// File texts as they were when the step started.
    snapshot: FileSnapshot,
}

impl std::fmt::Debug for RaSemanticProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RaSemanticProvider::{:?}", self.mode)
    }
}

impl RaSemanticProvider {
    pub fn file_scope() -> Self {
        RaSemanticProvider {
            flow: RsFlow::new(None),
            mode: ProviderMode::FileScope,
            snapshot: FileSnapshot::new(),
        }
    }

    pub fn workspace_scope(workspace_root: PathBuf) -> Self {
        RaSemanticProvider {
            flow: RsFlow::new(Some(workspace_root)),
            mode: ProviderMode::WorkspaceScope,
            snapshot: FileSnapshot::new(),
        }
    }

    pub fn clear_cache(&self) {
        self.flow.clear();
    }

    /// The text queries about `file_path` read: pinned on first read, so a
    /// step's answers never see what the step writes.
    fn read_file(&self, file_path: &Path) -> SemanticResult<Arc<str>> {
        self.snapshot.get_or_read(file_path, || {
            std::fs::read_to_string(file_path).map_err(|e| SemanticError::FileRead {
                path: file_path.to_path_buf(),
                message: e.to_string(),
            })
        })
    }

    /// Read `file_path` at its pinned text, then ask rust-analyzer.
    fn ask<T>(
        &self,
        file_path: &Path,
        f: impl FnOnce(&crate::workspace::RaWorkspace, &Path) -> T,
    ) -> SemanticResult<T> {
        let content = self.read_file(file_path)?;
        self.flow.update(file_path, &content);
        self.flow.with_workspace(file_path, f)
    }
}

impl SemanticProvider for RaSemanticProvider {
    fn get_definition(
        &self,
        file_path: &Path,
        range: ByteRange,
        _options: DefinitionOptions,
    ) -> SemanticResult<Option<DefinitionResult>> {
        self.ask(file_path, |ws, path| symbols::definition(ws, path, range))
    }

    fn find_references(
        &self,
        file_path: &Path,
        range: ByteRange,
    ) -> SemanticResult<ReferencesResult> {
        self.ask(file_path, |ws, path| symbols::references(ws, path, range))
    }

    fn get_type(&self, file_path: &Path, range: ByteRange) -> SemanticResult<Option<String>> {
        self.ask(file_path, |ws, path| symbols::type_of(ws, path, range))
    }

    fn get_data_flow(
        &self,
        file_path: &Path,
        range: ByteRange,
        options: FlowOptions,
    ) -> SemanticResult<FlowResult> {
        let content = self.read_file(file_path)?;
        self.flow.data_flow(file_path, &content, range, options)
    }

    fn get_call_graph(
        &self,
        file_path: &Path,
        range: ByteRange,
        options: CallGraphOptions,
    ) -> SemanticResult<CallGraphResult> {
        let content = self.read_file(file_path)?;
        self.flow.call_graph(file_path, &content, range, options)
    }

    fn get_mutations(
        &self,
        file_path: &Path,
        range: ByteRange,
        options: MutationOptions,
    ) -> SemanticResult<MutationsResult> {
        let content = self.read_file(file_path)?;
        self.flow.mutations(file_path, &content, range, options)
    }

    fn resolve_module(&self, from_file: &Path, specifier: &str) -> SemanticResult<Option<PathBuf>> {
        self.ask(from_file, |ws, path| {
            modules::resolve_module(ws, path, specifier)
        })
    }

    fn specifier_for(
        &self,
        from_file: &Path,
        target: &Path,
        _options: &SpecifierOptions,
    ) -> SemanticResult<Option<String>> {
        let target = target
            .canonicalize()
            .unwrap_or_else(|_| target.to_path_buf());
        self.ask(from_file, |ws, path| {
            modules::specifier_for(ws, path, &target)
        })
    }

    fn importers(&self, target: &Path) -> SemanticResult<ImportersResult> {
        self.ask(target, modules::importers)
    }

    fn set_entry_point_rules(&self, rules: Vec<EntryPointRule>) {
        self.flow.set_entry_point_rules(rules);
    }

    fn get_entry_points(
        &self,
        file_path: &Path,
        range: ByteRange,
    ) -> SemanticResult<Vec<EntryPoint>> {
        let content = self.read_file(file_path)?;
        self.flow.entry_points(file_path, &content, range)
    }

    fn list_entry_points(&self, _language: &str) -> SemanticResult<Vec<FunctionEntryPoints>> {
        Ok(self.flow.all_entry_points())
    }

    fn snapshot_content(&self, file_path: &Path) -> Option<Arc<str>> {
        self.read_file(file_path).ok()
    }

    fn notify_file_processed(&self, file_path: &Path, content: &str) -> SemanticResult<()> {
        self.snapshot.pin(file_path, content);
        self.flow.update(file_path, content);
        Ok(())
    }

    fn supports_language(&self, lang: &str) -> bool {
        matches!(lang.to_lowercase().as_str(), "rust" | "rs")
    }

    fn mode(&self) -> ProviderMode {
        self.mode
    }
}
