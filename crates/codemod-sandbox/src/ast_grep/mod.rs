pub(crate) mod sg_node;
mod types;
mod utils;

#[cfg(feature = "wasm")]
pub mod wasm_lang;

#[cfg(feature = "wasm")]
pub mod wasm_utils;

#[cfg(feature = "native")]
pub mod native;

#[cfg(all(not(feature = "wasm"), not(feature = "native")))]
use ast_grep_language::{LanguageExt, SupportLang};

#[cfg(feature = "native")]
use crate::sandbox::engine::codemod_lang::CodemodLang;
#[cfg(feature = "native")]
use ast_grep_core::tree_sitter::LanguageExt;

#[cfg(feature = "wasm")]
use ast_grep_core::language::Language;

use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{prelude::Func, Class, Ctx, Exception, Object, Result};
#[cfg(feature = "native")]
use rquickjs::{Function, Value};

use sg_node::{SgNodeRjs, SgRootRjs};

pub(crate) mod scanner;
pub(crate) mod serde;

#[cfg(feature = "native")]
pub use native::{scan_file_with_combined_scan, with_combined_scan};

#[allow(dead_code)]
pub(crate) struct AstGrepModule;

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

pub(crate) fn parse_rjs(ctx: Ctx<'_>, lang: String, src: String) -> Result<SgRootRjs<'_>> {
    SgRootRjs::try_new(lang, src, None)
        .map_err(|e| Exception::throw_message(&ctx, &format!("Failed to parse: {e}")))
}

fn parse_async_rjs(ctx: Ctx<'_>, lang: String, src: String) -> Result<SgRootRjs<'_>> {
    #[cfg(feature = "wasm")]
    {
        if !wasm_lang::WasmLang::is_parser_initialized() {
            return Err(Exception::throw_message(&ctx, "Tree-sitter parser not initialized. Ensure setupParser() has completed before calling parseAsync."));
        }
    }

    // Call the same implementation as parse_rjs since the async setup should be done by now
    SgRootRjs::try_new(lang, src, None)
        .map_err(|e| Exception::throw_message(&ctx, &format!("Failed to parse: {e}")))
}

#[cfg(feature = "native")]
fn parse_file_rjs(ctx: Ctx<'_>, lang: String, file_path: String) -> Result<SgRootRjs<'_>> {
    let file_content = std::fs::read_to_string(file_path.clone())
        .map_err(|e| Exception::throw_message(&ctx, &format!("Failed to read file: {e}")))?;
    SgRootRjs::try_new(lang, file_content, Some(file_path))
        .map_err(|e| Exception::throw_message(&ctx, &format!("Failed to parse: {e}")))
}

// Corresponds to the `kind` function in wasm/lib.rs
// Takes lang: string, kind_name: string -> u16
#[cfg(feature = "wasm")]
fn kind_rjs(ctx: Ctx<'_>, lang: String, kind_name: String) -> Result<u16> {
    use std::str::FromStr;

    use wasm_lang::WasmLang;
    let lang = WasmLang::from_str(&lang)
        .map_err(|e| Exception::throw_message(&ctx, &format!("Language error: {}", e)))?;

    let kind = lang.kind_to_id(&kind_name);
    Ok(kind)
}

#[cfg(all(not(feature = "wasm"), not(feature = "native")))]
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
    use crate::sandbox::engine::ExecutionModeFlag;
    use crate::utils::quickjs_utils::maybe_promise;
    use std::str::FromStr;

    let should_noop = ctx
        .userdata::<ExecutionModeFlag>()
        .map(|f| f.test_mode)
        .unwrap_or(true); // No flag = in-memory engine → no-op
    if should_noop {
        let ctx2 = ctx.clone();
        let promise = rquickjs::Promise::wrap_future(&ctx, async move {
            Ok::<_, rquickjs::Error>(Value::new_undefined(ctx2))
        })?;
        return Ok(promise.into_value());
    }

    let file_path = std::path::Path::new(&path_to_file);

    // Read the file
    let content = std::fs::read_to_string(file_path).map_err(|e| {
        Exception::throw_message(
            &ctx,
            &format!("Failed to read file '{}': {}", path_to_file, e),
        )
    })?;

    // Parse with language and filename
    let sg_root = SgRootRjs::try_new(language, content.clone(), Some(path_to_file.clone()))
        .map_err(|e| Exception::throw_message(&ctx, &format!("Failed to parse: {e}")))?;

    // Build default options object
    let options = Object::new(ctx.clone())?;
    options.set("params", Object::new(ctx.clone())?)?;

    let lang_str = CodemodLang::from_str(
        sg_root.inner.grep.lang().to_string().as_str(),
    )
    .map(|l| l.to_string())
    .unwrap_or_default();
    options.set("language", lang_str)?;

    // Call the transform function
    let result_val: Value<'js> = transform_fn.call((sg_root.clone(), options))?;

    // Create a promise to handle async transforms
    let ctx2 = ctx.clone();
    let promise = rquickjs::Promise::wrap_future(&ctx, async move {
        let result = maybe_promise(result_val).await.map_err(|e| {
            Exception::throw_message(&ctx2, &format!("Transform failed: {e}"))
        })?;

        let rename_to = sg_root.get_rename_to();

        if result.is_string() {
            let new_content: String = result.get().unwrap();
            let write_path = rename_to.as_deref().unwrap_or(&path_to_file);

            std::fs::write(write_path, &new_content).map_err(|e| {
                Exception::throw_message(
                    &ctx2,
                    &format!("Failed to write file '{}': {}", write_path, e),
                )
            })?;

            // If renamed, delete the original file
            if rename_to.is_some() && write_path != path_to_file {
                let _ = std::fs::remove_file(&path_to_file);
            }
        } else if result.is_null() || result.is_undefined() {
            // If rename was requested with null return, rename with original content
            if let Some(ref new_path) = rename_to {
                std::fs::write(new_path, &content).map_err(|e| {
                    Exception::throw_message(
                        &ctx2,
                        &format!("Failed to write file '{}': {}", new_path, e),
                    )
                })?;

                if new_path != &path_to_file {
                    let _ = std::fs::remove_file(&path_to_file);
                }
            }
            // No rename and null return = no changes
        }

        Ok::<_, rquickjs::Error>(Value::new_undefined(ctx2))
    })?;

    Ok(promise.into_value())
}
