use crate::ast_grep::serde::JsValue;
use crate::ast_grep::sg_core::CoreRoot;
use crate::ast_grep::sg_node::SgNodeRjs;
use crate::sandbox::engine::execution_engine::{ExecutionResult, ModifiedResult};
use crate::sandbox::errors::ExecutionError;
use rquickjs::{Ctx, FromJs, IntoJs, Object, Value};
use std::collections::HashMap;
use std::path::PathBuf;

/// How to check whether content was modified
pub enum ModificationCheck<'a> {
    /// Compare new content string against original content string
    StringEquality { original_content: &'a str },
    /// Compare SHA256 hash of new content against original hash
    #[cfg(feature = "native")]
    Sha256(Option<[u8; 32]>),
}

/// The error for an object result without an `output` field.
pub const MISSING_OUTPUT_MESSAGE: &str = "Object codemod results must contain an 'output' field";

/// The error for a `jssgTransform()` inner transform returning an object.
pub const SECONDARY_STRUCTURED_RESULT: &str = "jssgTransform() transforms must return a string or null; structured { content, output } results are only supported from the top-level transform";

fn execution_failed(message: String) -> ExecutionError {
    ExecutionError::Runtime {
        source: crate::sandbox::errors::RuntimeError::ExecutionFailed { message },
    }
}

/// An object result without an `output` field.
pub fn missing_output_error() -> ExecutionError {
    execution_failed(MISSING_OUTPUT_MESSAGE.to_string())
}

/// An `output` field that could not be read as JSON.
pub fn output_json_error(error: impl std::fmt::Display) -> ExecutionError {
    execution_failed(format!("Codemod output must be JSON: {error}"))
}

/// A transform's (awaited) content as plain data, read from the JavaScript
/// value by a binding (this rquickjs facade, or the orchestration QuickJS
/// host's), so the result rules in
/// [`classify_transform_result`] are binding-agnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReturnedContent {
    Text(String),
    /// `null` or `undefined`.
    Nullish,
    /// Anything else, by its QuickJS type name (`int`, `float`, `bool`,
    /// `object`, ...), which the error message shows.
    Other(String),
}

impl ReturnedContent {
    pub(crate) fn from_value(value: &Value<'_>) -> Self {
        if value.is_string() {
            match value.get::<String>() {
                Ok(text) => ReturnedContent::Text(text),
                Err(_) => ReturnedContent::Other(value.type_name().to_string()),
            }
        } else if value.is_null() || value.is_undefined() {
            ReturnedContent::Nullish
        } else {
            ReturnedContent::Other(value.type_name().to_string())
        }
    }
}

/// Split the extended `{ content, output }` result without changing legacy
/// string/null result handling.
pub fn extract_transform_output<'js>(
    ctx: &Ctx<'js>,
    result: Value<'js>,
) -> Result<(Value<'js>, Option<serde_json::Value>), ExecutionError> {
    let Some(object) = result.as_object() else {
        return Ok((result, None));
    };
    let output: Value<'_> = object
        .get("output")
        .map_err(|error| execution_failed(error.to_string()))?;
    if output.is_undefined() {
        return Err(missing_output_error());
    }
    let content: Value<'_> = object
        .get("content")
        .map_err(|error| execution_failed(error.to_string()))?;
    let output = JsValue::from_js(ctx, output).map_err(output_json_error)?.0;
    Ok((content, Some(output)))
}

/// Build the JS `options` object passed to the transform function.
///
/// Creates an object with: `{ params, language, matches, matrixValues, dryRun, targetDir }`
pub fn build_transform_options<'js>(
    ctx: &Ctx<'js>,
    params: HashMap<String, serde_json::Value>,
    language: &str,
    matrix_values: Option<HashMap<String, serde_json::Value>>,
    matches: Option<Vec<SgNodeRjs<'js>>>,
    dry_run: bool,
    target_dir: &str,
) -> Result<Value<'js>, ExecutionError> {
    let init_failed = |e: rquickjs::Error| ExecutionError::Runtime {
        source: crate::sandbox::errors::RuntimeError::InitializationFailed {
            message: e.to_string(),
        },
    };
    let run_options = Object::new(ctx.clone()).map_err(init_failed)?;

    let params_js = params
        .into_iter()
        .map(|(k, v)| (k, JsValue(v)))
        .collect::<HashMap<String, JsValue>>();
    run_options.set("params", params_js).map_err(init_failed)?;
    run_options.set("language", language).map_err(init_failed)?;
    run_options.set("matches", matches).map_err(init_failed)?;

    let matrix_values_js = matrix_values.map(|input| {
        input
            .into_iter()
            .map(|(k, v)| (k, JsValue(v)))
            .collect::<HashMap<String, JsValue>>()
    });
    run_options
        .set("matrixValues", matrix_values_js)
        .map_err(init_failed)?;
    run_options.set("dryRun", dry_run).map_err(init_failed)?;
    run_options
        .set("targetDir", target_dir)
        .map_err(init_failed)?;

    run_options.into_js(ctx).map_err(init_failed)
}

/// Process the result value returned by a transform function.
///
/// Handles string results, null/undefined, and rename_to logic.
/// Uses `ModificationCheck` to determine whether content was actually modified.
pub fn process_transform_result(
    result_obj: &Value<'_>,
    sg_root: &CoreRoot,
    modification_check: ModificationCheck<'_>,
) -> Result<ExecutionResult, ExecutionError> {
    classify_transform_result(
        ReturnedContent::from_value(result_obj),
        sg_root,
        modification_check,
    )
}

/// The result rules every JSSG runner shares, binding-agnostic: a string is
/// the new content (modified when it differs or the file was renamed),
/// `null`/`undefined` keeps the file (modified only by a `rename()`, with
/// the original content), and anything else is an error.
pub fn classify_transform_result(
    content: ReturnedContent,
    sg_root: &CoreRoot,
    modification_check: ModificationCheck<'_>,
) -> Result<ExecutionResult, ExecutionError> {
    let rename_to = sg_root.rename_to().map(PathBuf::from);

    match content {
        ReturnedContent::Text(new_content) => {
            let is_modified = match modification_check {
                ModificationCheck::StringEquality { original_content } => {
                    new_content != original_content
                }
                #[cfg(feature = "native")]
                ModificationCheck::Sha256(original_sha256) => match original_sha256 {
                    Some(original_hash) => {
                        use sha2::{Digest, Sha256};
                        let mut hasher = Sha256::new();
                        hasher.update(new_content.as_bytes());
                        let new_hash: [u8; 32] = hasher.finalize().into();
                        new_hash != original_hash
                    }
                    None => true,
                },
            };
            if is_modified || rename_to.is_some() {
                Ok(ExecutionResult::Modified(ModifiedResult {
                    content: new_content,
                    rename_to,
                }))
            } else {
                Ok(ExecutionResult::Unmodified)
            }
        }
        ReturnedContent::Nullish => {
            if rename_to.is_some() {
                let original_content = sg_root.source();
                Ok(ExecutionResult::Modified(ModifiedResult {
                    content: original_content,
                    rename_to,
                }))
            } else {
                Ok(ExecutionResult::Unmodified)
            }
        }
        ReturnedContent::Other(type_name) => Err(execution_failed(format!(
            "Codemod transform functions must return either a string or null/undefined. Received {type_name}"
        ))),
    }
}
