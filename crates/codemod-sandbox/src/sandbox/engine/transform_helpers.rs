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
/// Creates an object with: `{ params, language, matches, matrixValues }`
pub fn build_transform_options<'js>(
    ctx: &Ctx<'js>,
    params: HashMap<String, serde_json::Value>,
    language: &str,
    matrix_values: Option<HashMap<String, serde_json::Value>>,
    matches: Option<Vec<SgNodeRjs<'js>>>,
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
        .into_js(ctx)
        .map_err(|e| ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::InitializationFailed {
                message: e.to_string(),
            },
        })
}

/// Process the result value returned by a transform function.
///
/// Handles string results, null/undefined, and rename_to logic.
/// Uses `ModificationCheck` to determine whether content was actually modified.
pub fn process_transform_result(
    result_obj: &Value<'_>,
    sg_root_inner: &Arc<SgRootInner>,
    modification_check: ModificationCheck<'_>,
) -> Result<ExecutionResult, ExecutionError> {
    let rename_to = sg_root_inner
        .rename_to
        .lock()
        .unwrap()
        .clone()
        .map(PathBuf::from);

    if result_obj.is_string() {
        let new_content = result_obj.get::<String>().unwrap();
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
        Err(ExecutionError::Runtime {
            source: crate::sandbox::errors::RuntimeError::ExecutionFailed {
                message: "Invalid result type".to_string(),
            },
        })
    }
}
