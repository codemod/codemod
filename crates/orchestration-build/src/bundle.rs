//! rolldown bundles: one per `jssg()` transform artifact, and the QuickJS
//! bundle of the whole workflow graph.
//!
//! The workflow bundle resolves `@codemod.com/orchestration` (and
//! `/portable`) to the portable runtime and rejects every other entry point
//! of the package and every `node:*` import. `codemod:*` stays external: the
//! host provides it. Async functions and generators are lowered to
//! generators, so continuations go through `Promise.prototype.then`, which
//! carries the orchestration async context in QuickJS.
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

use arcstr::ArcStr;
use oxc::{
    allocator::Allocator,
    codegen::Codegen,
    parser::Parser,
    semantic::SemanticBuilder,
    span::SourceType,
    transformer::{HelperLoaderMode, TransformOptions, Transformer},
};
use rolldown::{Bundler, BundlerOptions, InputItem, OutputFormat, Platform, ResolveOptions};
use rolldown_common::{ModuleType, Output, ResolvedExternal};
use rolldown_plugin::{
    __inner::SharedPluginable, HookLoadArgs, HookLoadOutput, HookLoadReturn, HookResolveIdArgs,
    HookResolveIdOutput, HookResolveIdReturn, HookTransformArgs, HookTransformOutput,
    HookTransformReturn, HookUsage, Plugin, PluginContext, SharedTransformPluginContext,
};

use crate::{
    sha256,
    split::{build_module, JssgArtifact},
    BuildError,
};

const PACKAGE: &str = "@codemod.com/orchestration";
const ARTIFACTS: &str = "__CODEMOD_QUICKJS_ARTIFACTS__";
const ENTRY_ID: &str = "\0codemod-quickjs-entry";

/// Where the portable orchestration runtime's sources are.
#[derive(Debug, Clone)]
pub struct Runtime {
    /// The orchestration package's `src` directory.
    pub src: PathBuf,
}

impl Runtime {
    fn portable(&self) -> PathBuf {
        self.src.join("portable.ts")
    }

    fn entry(&self) -> PathBuf {
        self.src.join("host").join("quickjs").join("entry.ts")
    }
}

#[derive(Debug, Clone)]
pub struct QuickjsBundle {
    /// The ES module text.
    pub code: String,
    /// SHA-256 (hex) of `code`.
    pub hash: String,
    /// Sorted by hash.
    pub artifacts: Vec<JssgArtifact>,
    /// Every source file that went into the bundle. Sorted.
    pub inputs: Vec<PathBuf>,
}

pub(crate) struct BundledArtifact {
    pub code: String,
    pub inputs: Vec<PathBuf>,
}

fn module_type(file: &str) -> ModuleType {
    match Path::new(file).extension().and_then(|e| e.to_str()) {
        Some("tsx") => ModuleType::Tsx,
        Some("jsx") => ModuleType::Jsx,
        Some("js" | "mjs" | "cjs") => ModuleType::Js,
        _ => ModuleType::Ts,
    }
}

fn is_script(id: &str) -> bool {
    matches!(
        Path::new(id).extension().and_then(|e| e.to_str()),
        Some("ts" | "mts" | "cts" | "tsx" | "js" | "mjs" | "cjs" | "jsx")
    )
}

fn external(specifier: &str) -> HookResolveIdOutput {
    HookResolveIdOutput {
        id: specifier.into(),
        external: Some(ResolvedExternal::Bool(true)),
        ..Default::default()
    }
}

/// Errors raised inside hooks, kept whole: rolldown would flatten them into
/// a generic plugin error.
#[derive(Default)]
struct Failure(Mutex<Option<BuildError>>);

impl Failure {
    fn raise(&self, error: BuildError) -> anyhow::Error {
        let message = error.to_string();
        self.0.lock().unwrap().get_or_insert(error);
        anyhow::anyhow!(message)
    }

    fn take(&self) -> Option<BuildError> {
        self.0.lock().unwrap().take()
    }
}

// --- transform artifacts ------------------------------------------------------

/// The virtual entry stands in for the workflow module itself (same path), so
/// its relative imports resolve from the module's directory and the bundle's
/// region comments name it relative to that directory.
#[derive(Debug)]
struct ArtifactEntry {
    id: String,
    code: String,
}

impl Plugin for ArtifactEntry {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("codemod-jssg-artifact")
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::ResolveId | HookUsage::Load
    }

    async fn resolve_id(
        &self,
        _ctx: &PluginContext,
        args: &HookResolveIdArgs<'_>,
    ) -> HookResolveIdReturn {
        if args.specifier == self.id {
            return Ok(Some(HookResolveIdOutput::from_id(self.id.as_str())));
        }
        if args.specifier.starts_with("codemod:") {
            return Ok(Some(external(args.specifier)));
        }
        Ok(None)
    }

    async fn load(&self, _ctx: &PluginContext, args: &HookLoadArgs<'_>) -> HookLoadReturn {
        if args.id != self.id {
            return Ok(None);
        }
        Ok(Some(HookLoadOutput {
            code: ArcStr::from(self.code.as_str()),
            module_type: Some(module_type(&self.id)),
            ..Default::default()
        }))
    }
}

/// Bundle one transform's virtual entry into an ES module. The output names
/// no absolute path (comments are relative to the module's real directory),
/// so its hash is stable across checkouts. Node built-ins and `codemod:*`
/// stay external; everything else is inlined.
pub(crate) async fn bundle_artifact(
    entry: &str,
    file: &Path,
) -> Result<BundledArtifact, BuildError> {
    let directory = file
        .parent()
        .map(|parent| std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf()))
        .unwrap_or_default();
    let id = directory
        .join(file.file_name().unwrap_or_default())
        .to_string_lossy()
        .into_owned();
    let fail = |detail: String| BuildError {
        file: file.to_path_buf(),
        position: None,
        message: format!("failed to bundle a jssg transform: {detail}"),
    };
    let plugin: SharedPluginable = Arc::new(ArtifactEntry {
        id: id.clone(),
        code: entry.to_owned(),
    });
    let chunk = generate(
        BundlerOptions {
            input: Some(vec![InputItem {
                name: Some("transform".into()),
                import: id.clone(),
            }]),
            cwd: Some(directory),
            format: Some(OutputFormat::Esm),
            platform: Some(Platform::Node),
            ..Default::default()
        },
        vec![plugin],
    )
    .await
    .map_err(fail)?;
    let mut inputs = vec![file.to_path_buf()];
    inputs.extend(
        chunk
            .module_ids
            .iter()
            .filter(|module| **module != id)
            .filter_map(|module| source_file(module)),
    );
    Ok(BundledArtifact {
        code: chunk.code,
        inputs,
    })
}

struct Chunk {
    code: String,
    module_ids: Vec<String>,
}

async fn generate(
    options: BundlerOptions,
    plugins: Vec<SharedPluginable>,
) -> Result<Chunk, String> {
    let mut bundler = Bundler::with_plugins(options, plugins).map_err(|e| e.to_string())?;
    let output = bundler.generate().await.map_err(|e| e.to_string())?;
    if let Some(unresolved) = output
        .warnings
        .iter()
        .find(|warning| warning.kind().to_string() == "UNRESOLVED_IMPORT")
    {
        return Err(unresolved.to_string());
    }
    output
        .assets
        .into_iter()
        .find_map(|asset| match asset {
            Output::Chunk(chunk) => Some(Chunk {
                code: chunk.code.clone(),
                module_ids: chunk.module_ids.iter().map(|id| id.to_string()).collect(),
            }),
            Output::Asset(_) => None,
        })
        .ok_or_else(|| "rolldown produced no chunk".to_owned())
}

// --- the workflow bundle --------------------------------------------------------

#[derive(Default)]
struct Collected {
    artifacts: BTreeMap<String, JssgArtifact>,
    inputs: BTreeSet<PathBuf>,
}

struct QuickjsPlugin {
    runtime: Runtime,
    workflow_dir: PathBuf,
    entry: String,
    collected: Mutex<Collected>,
    failure: Failure,
}

impl std::fmt::Debug for QuickjsPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuickjsPlugin").finish_non_exhaustive()
    }
}

impl QuickjsPlugin {
    fn portable(&self) -> HookResolveIdReturn {
        Ok(Some(HookResolveIdOutput::from_id(
            self.runtime.portable().to_string_lossy().as_ref(),
        )))
    }
}

impl Plugin for QuickjsPlugin {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("codemod-quickjs")
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::ResolveId | HookUsage::Load | HookUsage::Transform
    }

    async fn resolve_id(
        &self,
        _ctx: &PluginContext,
        args: &HookResolveIdArgs<'_>,
    ) -> HookResolveIdReturn {
        let specifier = args.specifier;
        if specifier == ENTRY_ID {
            return Ok(Some(HookResolveIdOutput::from_id(ENTRY_ID)));
        }
        if specifier == PACKAGE || specifier == format!("{PACKAGE}/portable") {
            return self.portable();
        }
        if specifier.starts_with(&format!("{PACKAGE}/")) {
            anyhow::bail!(
                "'{specifier}' is not available in the quickjs runtime; import '{PACKAGE}' (the portable API)"
            );
        }
        if specifier.starts_with("node:") {
            anyhow::bail!(
                "'{specifier}' (imported by {}) is a Node module; the quickjs runtime has no Node APIs",
                args.importer.unwrap_or("the workflow")
            );
        }
        if specifier.starts_with("codemod:") {
            return Ok(Some(external(specifier)));
        }
        // The package's Node entry, reached by a relative import from inside
        // the package (tests, demos), is the portable API as well.
        if specifier.starts_with('.') || Path::new(specifier).is_absolute() {
            let base = args
                .importer
                .filter(|importer| *importer != ENTRY_ID)
                .and_then(|importer| Path::new(importer).parent())
                .unwrap_or(&self.workflow_dir);
            let target = normalize(&base.join(specifier));
            if target == self.runtime.src.join("index")
                || target == self.runtime.src.join("index.ts")
            {
                return self.portable();
            }
        }
        Ok(None)
    }

    async fn load(&self, _ctx: &PluginContext, args: &HookLoadArgs<'_>) -> HookLoadReturn {
        if args.id == ENTRY_ID {
            return Ok(Some(HookLoadOutput {
                code: ArcStr::from(self.entry.as_str()),
                module_type: Some(ModuleType::Js),
                ..Default::default()
            }));
        }
        let path = Path::new(args.id);
        // The package's own sources define `jssg`; they never call it.
        if args.id.starts_with('\0') || !is_script(args.id) || path.starts_with(&self.runtime.src) {
            return Ok(None);
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            return Ok(None);
        };
        if !text.contains("jssg(") {
            return Ok(None);
        }
        let built = build_module(&text, path)
            .await
            .map_err(|error| self.failure.raise(error))?;
        let mut collected = self.collected.lock().unwrap();
        for artifact in built.artifacts {
            collected.artifacts.insert(artifact.hash.clone(), artifact);
        }
        collected.inputs.extend(built.inputs);
        Ok(Some(HookLoadOutput {
            code: ArcStr::from(built.source),
            module_type: Some(module_type(args.id)),
            ..Default::default()
        }))
    }

    async fn transform(
        &self,
        _ctx: SharedTransformPluginContext,
        args: &HookTransformArgs<'_>,
    ) -> HookTransformReturn {
        if !matches!(
            args.module_type,
            ModuleType::Js | ModuleType::Jsx | ModuleType::Ts | ModuleType::Tsx
        ) {
            return Ok(None);
        }
        let id = if args.id == ENTRY_ID {
            "entry.js"
        } else {
            args.id
        };
        let code = lower_async(args.code, id, args.module_type)
            .map_err(|message| anyhow::anyhow!("{id}: {message}"))?;
        Ok(Some(HookTransformOutput {
            code: Some(code),
            module_type: Some(ModuleType::Js),
            ..Default::default()
        }))
    }
}

/// Strip types and lower async functions and generators (nothing else: the
/// rest of ES2022 runs as written).
fn lower_async(code: &str, id: &str, module_type: &ModuleType) -> Result<String, String> {
    let source_type = match module_type {
        ModuleType::Ts => SourceType::ts(),
        ModuleType::Tsx => SourceType::tsx(),
        ModuleType::Jsx => SourceType::jsx(),
        _ => SourceType::mjs(),
    };
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, code, source_type).parse();
    if let Some(error) = parsed.errors.first() {
        return Err(error.message.to_string());
    }
    let mut program = parsed.program;
    let scoping = SemanticBuilder::new()
        .build(&program)
        .semantic
        .into_scoping();
    let mut options = TransformOptions::default();
    options.helper_loader.mode = HelperLoaderMode::Runtime;
    options.env.es2017.async_to_generator = true;
    options.env.es2018.async_generator_functions = true;
    let result = Transformer::new(&allocator, Path::new(id), &options)
        .build_with_scoping(scoping, &mut program);
    if let Some(error) = result.errors.first() {
        return Err(error.message.to_string());
    }
    Ok(Codegen::new().build(&program).code)
}

/// A module id that names a file (not a virtual module such as rolldown's
/// runtime or an oxc helper).
fn source_file(id: &str) -> Option<PathBuf> {
    Path::new(id).is_absolute().then(|| PathBuf::from(id))
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Build the QuickJS bundle for `workflow` (an absolute path).
pub async fn build_quickjs_bundle(
    workflow: &Path,
    runtime: &Runtime,
) -> Result<QuickjsBundle, BuildError> {
    let workflow_dir = workflow.parent().unwrap_or(Path::new("/")).to_path_buf();
    // The QuickJS entry first, so the host and the async context are
    // installed before the workflow's modules evaluate.
    let entry = format!(
        "import {{ runBundled }} from {};\nimport * as workflow from {};\nexport function main(request) {{ return runBundled(workflow, {ARTIFACTS}, request); }}\n",
        json(&runtime.entry().to_string_lossy()),
        json(&workflow.to_string_lossy()),
    );
    let plugin = Arc::new(QuickjsPlugin {
        runtime: runtime.clone(),
        workflow_dir: workflow_dir.clone(),
        entry,
        collected: Mutex::default(),
        failure: Failure::default(),
    });
    let result = generate(
        BundlerOptions {
            input: Some(vec![InputItem {
                name: Some("workflow".into()),
                import: ENTRY_ID.into(),
            }]),
            cwd: Some(workflow_dir),
            format: Some(OutputFormat::Esm),
            platform: Some(Platform::Neutral),
            resolve: Some(ResolveOptions {
                main_fields: Some(vec!["module".into(), "main".into()]),
                ..Default::default()
            }),
            ..Default::default()
        },
        vec![plugin.clone() as SharedPluginable],
    )
    .await;
    if let Some(error) = plugin.failure.take() {
        return Err(error);
    }
    let chunk = result.map_err(|detail| BuildError {
        file: workflow.to_path_buf(),
        position: None,
        message: format!("failed to build the quickjs bundle: {detail}"),
    })?;
    let collected = std::mem::take(&mut *plugin.collected.lock().unwrap());
    let table: serde_json::Map<String, serde_json::Value> = collected
        .artifacts
        .values()
        .map(|artifact| {
            (
                artifact.hash.clone(),
                serde_json::json!({ "name": artifact.name, "source": artifact.source }),
            )
        })
        .collect();
    let table = serde_json::to_string(&table).expect("the artifact table serializes");
    let code = format!(
        "const {ARTIFACTS} = JSON.parse({});\n{}",
        json(&table),
        chunk.code
    );
    let mut inputs: BTreeSet<PathBuf> = chunk
        .module_ids
        .iter()
        .filter_map(|id| source_file(id))
        .collect();
    inputs.extend(collected.inputs);
    Ok(QuickjsBundle {
        hash: sha256(&[code.as_bytes()]),
        code,
        artifacts: collected.artifacts.into_values().collect(),
        inputs: inputs.into_iter().collect(),
    })
}

fn json(text: &str) -> String {
    serde_json::to_string(text).expect("a string serializes")
}
