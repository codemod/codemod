pub(crate) mod sg_node;
mod types;
pub(crate) mod utils;

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub mod wasm_lang;

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub mod wasm_utils;

#[cfg(feature = "native")]
pub mod native;

#[cfg(all(
    not(all(feature = "wasm", target_arch = "wasm32")),
    not(feature = "native")
))]
use ast_grep_language::{LanguageExt, SupportLang};

#[cfg(feature = "native")]
use ast_grep_core::tree_sitter::LanguageExt;

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
use ast_grep_core::language::Language;

#[cfg(feature = "native")]
use crate::{
    sandbox::engine::codemod_lang::CodemodLang,
    sandbox::engine::{
        execution_engine::{
            validate_path_within_target, DryRunExecutionFlag, FileChange, JssgExecutionContext,
            JssgFileChanges,
        },
        transform_helpers::{build_transform_options, process_transform_result, ModificationCheck},
        ExecutionModeFlag,
    },
    utils::quickjs_utils::maybe_promise,
};
use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{
    prelude::{Func, Opt},
    Class, Ctx, Exception, Function, Object, Result, Value,
};
#[cfg(feature = "native")]
use std::{str::FromStr, sync::Arc};

use sg_node::{SgNodeRjs, SgRootRjs};

pub(crate) mod scanner;
pub(crate) mod serde;

#[cfg(feature = "native")]
pub use native::{scan_file_with_combined_scan, with_combined_scan};

/// The `codemod:ast-grep` runtime module. Public so other hosts can register
/// it in their own QuickJS runtimes.
#[allow(dead_code)]
pub struct AstGrepModule;

impl ModuleDef for AstGrepModule {
    fn declare(declare: &Declarations) -> Result<()> {
        declare.declare(stringify!(SgRootRjs))?;
        declare.declare(stringify!(SgNodeRjs))?;
        declare.declare("parse")?;
        declare.declare("parseAsync")?;
        declare.declare("kind")?;
        declare.declare("default")?;
        #[cfg(feature = "native")]
        declare.declare("parseFile")?;
        #[cfg(feature = "native")]
        declare.declare("jssgTransform")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let default = Object::new(ctx.clone())?;
        Class::<SgRootRjs>::define(&default)?;
        Class::<SgNodeRjs>::define(&default)?;
        default.set("parse", Func::from(parse_rjs))?;
        default.set("parseAsync", Func::from(parse_async_rjs))?;
        default.set("kind", Func::from(kind_rjs))?;
        #[cfg(feature = "native")]
        {
            default.set("parseFile", Func::from(parse_file_rjs))?;
            exports.export("parseFile", Func::from(parse_file_rjs))?;
            default.set("jssgTransform", Func::from(jssg_transform_rjs))?;
            exports.export("jssgTransform", Func::from(jssg_transform_rjs))?;
        }
        exports.export("default", default)?;
        exports.export("parse", Func::from(parse_rjs))?;
        exports.export("parseAsync", Func::from(parse_async_rjs))?;
        exports.export("kind", Func::from(kind_rjs))?;
        Ok(())
    }
}

/// The language `parse(text)` and `parse(file)` use when the call does not
/// name one. Hosts that know it store this as userdata: a workflow's
/// `forEachFile` step sets it to the selector's `language`.
#[derive(Debug, Clone)]
pub struct DefaultParseLanguage(pub String);

unsafe impl<'js> rquickjs::JsLifetime<'js> for DefaultParseLanguage {
    type Changed<'to> = DefaultParseLanguage;
}

/// What a `parse(...)` call asks for, after sorting out its forms.
struct ParseRequest {
    language: String,
    text: String,
    filename: Option<String>,
}

/// `parse` accepts:
///
/// - `parse(lang, text)`: the original form;
/// - `parse(text)` and `parse(text, { language })`;
/// - `parse(file)` and `parse(file, { language })`, where `file` is an
///   object with `text` and optionally `path` and `language` (a workflow
///   `forEachFile` file). The root's `filename()` is then the file's path.
///
/// Without a language in the call, it comes from the file object, then the
/// host's [`DefaultParseLanguage`], then the file's extension.
fn parse_request<'js>(
    ctx: &Ctx<'js>,
    source: Value<'js>,
    second: Option<Value<'js>>,
) -> Result<ParseRequest> {
    let language_option = |value: &Value<'js>| -> Result<Option<String>> {
        match value.as_object() {
            Some(object) => object.get::<_, Option<String>>("language"),
            None => Ok(None),
        }
    };
    let default_language = || ctx.userdata::<DefaultParseLanguage>().map(|l| l.0.clone());

    if let Some(text) = source.as_string() {
        let text = text.to_string()?;
        if let Some(second) = second.as_ref().and_then(|v| v.as_string()) {
            // parse(lang, text)
            return Ok(ParseRequest {
                language: text,
                text: second.to_string()?,
                filename: None,
            });
        }
        let language = match &second {
            Some(value) => language_option(value)?,
            None => None,
        }
        .or_else(default_language)
        .ok_or_else(|| {
            Exception::throw_message(
                ctx,
                "parse(text) needs a language: parse(text, { language: \"tsx\" })",
            )
        })?;
        return Ok(ParseRequest {
            language,
            text,
            filename: None,
        });
    }

    let Some(object) = source.as_object() else {
        return Err(Exception::throw_type(
            ctx,
            "parse() expects source text or a file object, for example parse(file) or parse(text, { language })",
        ));
    };
    let Some(text) = object.get::<_, Option<String>>("text")? else {
        return Err(Exception::throw_type(
            ctx,
            "parse(file) expects an object with a string `text`, such as the file a forEachFile callback receives",
        ));
    };
    let path: Option<String> = object.get("path")?;
    let mut language = match &second {
        Some(value) => language_option(value)?,
        None => None,
    };
    if language.is_none() {
        language = object.get::<_, Option<String>>("language")?;
    }
    let language = language
        .or_else(default_language)
        .or_else(|| path.as_deref().and_then(language_from_path))
        .ok_or_else(|| {
            Exception::throw_message(
                ctx,
                &format!(
                    "parse(file): no language for '{}'; pass one with parse(file, {{ language }})",
                    path.as_deref().unwrap_or("<file>")
                ),
            )
        })?;
    Ok(ParseRequest {
        language,
        text,
        filename: path.map(|path| absolute_filename(ctx, path)),
    })
}

#[cfg(feature = "native")]
fn language_from_path(path: &str) -> Option<String> {
    use ast_grep_core::Language;
    CodemodLang::from_path(path).map(|lang| lang.to_string())
}

#[cfg(not(feature = "native"))]
fn language_from_path(_path: &str) -> Option<String> {
    None
}

/// A relative file path, made absolute against the run's target directory
/// so `filename()` and semantic lookups see a real path.
#[cfg(feature = "native")]
fn absolute_filename(ctx: &Ctx<'_>, path: String) -> String {
    let relative = std::path::Path::new(&path);
    if relative.is_absolute() {
        return path;
    }
    match ctx.userdata::<crate::sandbox::engine::execution_engine::TargetDirectory>() {
        Some(target) => target.0.join(relative).to_string_lossy().into_owned(),
        None => path,
    }
}

#[cfg(not(feature = "native"))]
fn absolute_filename(_ctx: &Ctx<'_>, path: String) -> String {
    path
}

fn parse_with<'js>(
    ctx: Ctx<'js>,
    source: Value<'js>,
    second: Opt<Value<'js>>,
) -> Result<SgRootRjs<'js>> {
    let request = parse_request(&ctx, source, second.0)?;
    #[cfg(feature = "native")]
    let target_directory = request.filename.as_ref().and_then(|_| {
        ctx.userdata::<crate::sandbox::engine::execution_engine::TargetDirectory>()
            .map(|guard| guard.0.clone())
    });
    #[cfg(not(feature = "native"))]
    let target_directory: Option<std::path::PathBuf> = None;
    SgRootRjs::try_new(
        request.language,
        request.text,
        request.filename,
        target_directory.as_deref(),
    )
    .map_err(|e| Exception::throw_message(&ctx, &format!("Failed to parse: {e}")))
}

pub(crate) fn parse_rjs<'js>(
    ctx: Ctx<'js>,
    source: Value<'js>,
    second: Opt<Value<'js>>,
) -> Result<SgRootRjs<'js>> {
    parse_with(ctx, source, second)
}

fn parse_async_rjs<'js>(
    ctx: Ctx<'js>,
    source: Value<'js>,
    second: Opt<Value<'js>>,
) -> Result<SgRootRjs<'js>> {
    #[cfg(all(feature = "wasm", target_arch = "wasm32"))]
    {
        if !wasm_lang::WasmLang::is_parser_initialized() {
            return Err(Exception::throw_message(&ctx, "Tree-sitter parser not initialized. Ensure setupParser() has completed before calling parseAsync."));
        }
    }

    // Same as parse: the async setup is done by now.
    parse_with(ctx, source, second)
}

#[cfg(feature = "native")]
fn parse_file_rjs(ctx: Ctx<'_>, lang: String, file_path: String) -> Result<SgRootRjs<'_>> {
    let file_content = std::fs::read_to_string(file_path.clone())
        .map_err(|e| Exception::throw_message(&ctx, &format!("Failed to read file: {e}")))?;
    SgRootRjs::try_new(lang, file_content, Some(file_path), None)
        .map_err(|e| Exception::throw_message(&ctx, &format!("Failed to parse: {e}")))
}

// Corresponds to the `kind` function in wasm/lib.rs
// Takes lang: string, kind_name: string -> u16
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
fn kind_rjs(ctx: Ctx<'_>, lang: String, kind_name: String) -> Result<u16> {
    use std::str::FromStr;

    use wasm_lang::WasmLang;
    let lang = WasmLang::from_str(&lang)
        .map_err(|e| Exception::throw_message(&ctx, &format!("Language error: {}", e)))?;

    let kind = lang.kind_to_id(&kind_name);
    Ok(kind)
}

#[cfg(all(
    not(all(feature = "wasm", target_arch = "wasm32")),
    not(feature = "native")
))]
fn kind_rjs(ctx: Ctx<'_>, lang: String, kind_name: String) -> Result<u16> {
    use std::str::FromStr;

    let lang = SupportLang::from_str(&lang)
        .map_err(|e| Exception::throw_message(&ctx, &format!("Language error: {e}")))?;

    let kind = lang
        .get_ts_language()
        .id_for_node_kind(&kind_name, /* named */ true);

    Ok(kind)
}

#[cfg(feature = "native")]
fn kind_rjs(ctx: Ctx<'_>, lang: String, kind_name: String) -> Result<u16> {
    use std::str::FromStr;

    let lang = CodemodLang::from_str(&lang)
        .map_err(|e| Exception::throw_message(&ctx, &format!("Language error: {e}")))?;

    let kind = lang
        .get_ts_language()
        .id_for_node_kind(&kind_name, /* named */ true);

    Ok(kind)
}

/// Execute a transform function on a file, writing back the result.
///
/// `jssgTransform(transformFn, pathToFile, language)` reads the file,
/// parses it, calls the transform, and writes back content + handles rename.
///
/// Returns a promise that resolves when the transform is complete.
#[cfg(feature = "native")]
fn jssg_transform_rjs<'js>(
    ctx: Ctx<'js>,
    transform_fn: Function<'js>,
    path_to_file: String,
    language: String,
) -> Result<Value<'js>> {
    let should_noop = ctx
        .userdata::<ExecutionModeFlag>()
        .map(|f| f.test_mode)
        .unwrap_or(true); // No flag = in-memory engine → no-op
    if should_noop {
        let ctx2 = ctx.clone();
        let promise = rquickjs::Promise::wrap_future(&ctx, async move {
            Ok::<_, rquickjs::Error>(Value::new_null(ctx2))
        })?;
        return Ok(promise.into_value());
    }

    let file_changes = ctx
        .userdata::<JssgFileChanges>()
        .map(|guard| guard.clone())
        .ok_or_else(|| Exception::throw_message(&ctx, "JssgFileChanges not found in userdata"))?;

    let exec_ctx = ctx.userdata::<JssgExecutionContext>();
    let params = exec_ctx
        .as_ref()
        .map(|c| c.params.clone())
        .unwrap_or_default();
    let matrix_values = exec_ctx.as_ref().and_then(|c| c.matrix_values.clone());
    let dry_run = ctx
        .userdata::<DryRunExecutionFlag>()
        .map(|flag| flag.0)
        .unwrap_or(false);

    let file_path = std::path::Path::new(&path_to_file);

    // Validate: file path must resolve within the target directory
    validate_path_within_target(&ctx, file_path, "jssgTransform()")?;

    // Read the file
    let content = std::fs::read_to_string(file_path).map_err(|e| {
        Exception::throw_message(
            &ctx,
            &format!("Failed to read file '{}': {}", path_to_file, e),
        )
    })?;

    // Parse with language and filename
    let target_directory = ctx
        .userdata::<crate::sandbox::engine::execution_engine::TargetDirectory>()
        .map(|guard| guard.0.clone());

    let sg_root = SgRootRjs::try_new(
        language,
        content.clone(),
        Some(path_to_file.clone()),
        target_directory.as_deref(),
    )
    .map_err(|e| Exception::throw_message(&ctx, &format!("Failed to parse: {e}")))?;

    let sg_root_inner = Arc::clone(&sg_root.inner);

    let lang_str = CodemodLang::from_str(sg_root.inner.grep.lang().to_string().as_str())
        .map(|l| l.to_string())
        .unwrap_or_default();

    let target_dir = target_directory
        .as_ref()
        .ok_or_else(|| Exception::throw_message(&ctx, "TargetDirectory not found in userdata"))?
        .to_string_lossy()
        .into_owned();
    let run_options = build_transform_options(
        &ctx,
        params,
        &lang_str,
        matrix_values,
        None,
        dry_run,
        &target_dir,
    )
    .map_err(|e| Exception::throw_message(&ctx, &format!("Failed to build options: {e}")))?;

    // Call the transform function
    let result_val: Value<'js> = transform_fn.call((sg_root, run_options))?;

    // Create a promise to handle async transforms
    let ctx2 = ctx.clone();
    let promise = rquickjs::Promise::wrap_future(&ctx, async move {
        let result = maybe_promise(result_val)
            .await
            .map_err(|e| Exception::throw_message(&ctx2, &format!("Transform failed: {e}")))?;

        let exec_result = process_transform_result(
            &result,
            &sg_root_inner,
            ModificationCheck::StringEquality {
                original_content: &content,
            },
        )
        .map_err(|e| Exception::throw_message(&ctx2, &format!("Transform result error: {e}")))?;

        // Extract content before pushing to accumulator
        let return_content = match &exec_result {
            crate::sandbox::engine::ExecutionResult::Modified(modified) => {
                Some(modified.content.clone())
            }
            _ => None,
        };

        // Push the file change to the shared accumulator instead of writing to disk
        let mut changes = file_changes.changes.lock().map_err(|e| {
            Exception::throw_message(&ctx2, &format!("Failed to lock file_changes mutex: {e}"))
        })?;
        changes.push(FileChange {
            path: std::path::PathBuf::from(&path_to_file),
            result: exec_result,
        });

        // Return the transformed content string, or null if unmodified
        match return_content {
            Some(content) => {
                Ok::<_, rquickjs::Error>(rquickjs::String::from_str(ctx2, &content)?.into_value())
            }
            None => Ok::<_, rquickjs::Error>(Value::new_null(ctx2)),
        }
    })?;

    Ok(promise.into_value())
}
