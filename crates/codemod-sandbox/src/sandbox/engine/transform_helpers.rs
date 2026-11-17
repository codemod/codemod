use crate::ast_grep::serde::JsValue;
use crate::ast_grep::sg_node::{SgNodeRjs, SgRootInner};
use crate::sandbox::engine::execution_engine::{ExecutionResult, ModifiedResult};
use crate::sandbox::errors::ExecutionError;
use rquickjs::{Ctx, IntoJs, Object, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// How to check whether content was modified
pub enum ModificationCheck<'a> {
    /// Compare new content string against original content string
    StringEquality { original_content: &'a str },
    /// Compare SHA256 hash of new content against original hash
    #[cfg(feature = "native")]
    Sha256(Option<[u8; 32]>),
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
    let run_options = Object::new(ctx.clone()).map_err(|e| ExecutionError::Runtime {
        source: crate::sandbox::errors::RuntimeError::InitializationFailed {
            message: e.to_string(),
        },
    })?;

    let params_js = params
        .into_iter()
        .map(|(k, v)| (k, JsValue(v)))
        .collect::<HashMap<String, JsValue>>();
    run_options
        .set("params", params_js)
        .map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: e.to_string(),
            },
        })?;

    run_options
        .set("language", language)
        .map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: e.to_string(),
            },
        })?;

    run_options
        .set("matches", matches)
        .map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: e.to_string(),
            },
        })?;

    let matrix_values_js = matrix_values.map(|input| {
        input
            .into_iter()
            .map(|(k, v)| (k, JsValue(v)))
            .collect::<HashMap<String, JsValue>>()
    });

    run_options
        .set("matrixValues", matrix_values_js)
        .map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: e.to_string(),
            },
        })?;

    run_options
        .set("dryRun", dry_run)
        .map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: e.to_string(),
            },
        })?;

    run_options
        .set("targetDir", target_dir)
        .map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: e.to_string(),
            },
        })?;

    run_options
        .into_js(ctx)
        .map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: e.to_string(),
            },
        })
}

/// Process the result value returned by a transform function: the new
/// content (a string), no change (`null` or `undefined`), or
/// `{ edit, data }`, where `edit` is either of those and `data` is any JSON
/// value the transform hands back for this file.
///
/// Uses `ModificationCheck` to determine whether content was actually modified.
pub fn process_transform_result(
    result_obj: &Value<'_>,
    sg_root_inner: &Arc<SgRootInner>,
    modification_check: ModificationCheck<'_>,
) -> Result<(ExecutionResult, Option<serde_json::Value>), ExecutionError> {
    let failed = |message: String| ExecutionError::Runtime {
        source: crate::sandbox::errors::RuntimeError::ExecutionFailed { message },
    };
    if let Some(object) = result_obj
        .as_object()
        .filter(|_| !result_obj.is_array() && !result_obj.is_function())
    {
        let ctx = result_obj.ctx();
        for key in object.keys::<String>() {
            let key = key.map_err(|e| failed(e.to_string()))?;
            if key != "edit" && key != "data" {
                return Err(failed(format!(
                    "A transform's result object takes only `edit` and `data`. Received `{key}`"
                )));
            }
        }
        // The edit is checked first, so a file that fails returns no data.
        let edit: Value = object.get("edit").map_err(|e| failed(e.to_string()))?;
        let result = process_edit(&edit, sg_root_inner, modification_check)?;
        let data: Value = object.get("data").map_err(|e| failed(e.to_string()))?;
        let data = if data.is_undefined() || data.is_null() {
            None
        } else {
            Some(data_json(ctx, data).map_err(failed)?)
        };
        return Ok((result, data));
    }
    Ok((
        process_edit(result_obj, sg_root_inner, modification_check)?,
        None,
    ))
}

/// `data` as JSON, the form it leaves the transform in.
fn data_json<'js>(ctx: &Ctx<'js>, data: Value<'js>) -> Result<serde_json::Value, String> {
    let type_name = data.type_name();
    let text = ctx
        .json_stringify(data)
        .map_err(|e| format!("A transform's `data` must be JSON: {}", caught(ctx, e)))?
        .ok_or_else(|| format!("A transform's `data` must be JSON. Received {type_name}"))?
        .to_string()
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// The message of a thrown JavaScript error.
fn caught(ctx: &Ctx<'_>, error: rquickjs::Error) -> String {
    if matches!(error, rquickjs::Error::Exception) {
        let thrown = ctx.catch();
        if let Some(message) = thrown
            .as_object()
            .and_then(|object| object.get::<_, String>("message").ok())
        {
            return message;
        }
    }
    error.to_string()
}

fn process_edit(
    result_obj: &Value<'_>,
    sg_root_inner: &Arc<SgRootInner>,
    modification_check: ModificationCheck<'_>,
) -> Result<ExecutionResult, ExecutionError> {
    let rename_to = sg_root_inner
        .rename_to
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .map(PathBuf::from);

    if result_obj.is_string() {
        let new_content = result_obj
            .get::<String>()
            .expect("result_obj should be a String after is_string() check");
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
    } else if result_obj.is_null() || result_obj.is_undefined() {
        if rename_to.is_some() {
            let original_content = sg_root_inner.grep.source().to_string();
            Ok(ExecutionResult::Modified(ModifiedResult {
                content: original_content,
                rename_to,
            }))
        } else {
            Ok(ExecutionResult::Unmodified)
        }
    } else {
        let type_name = result_obj.type_name();
        Err(ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::ExecutionFailed {
                message: format!("Codemod transform functions must return a string, null/undefined, or {{ edit, data }}. Received {type_name}"),
            },
        })
    }
}
