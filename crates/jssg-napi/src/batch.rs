//! The `node` runtime's JSSG batch: the bridge's own batch rules
//! ([`jssg_batch::BatchPlan`]) behind a napi class, driven by the portable
//! batch loop (`packages/orchestration/src/execution/jssg-batch.ts`).
//!
//! - `new JssgBatch(config, paths)`: hash-verified artifact, canonical target
//!   root, every path contained in it, language, compiled selector, params;
//!   semantic analysis refused (`unsupported in runtime 'node'`);
//! - `parse(start, contents)`: parses a chunk on worker threads (in
//!   parallel) and answers `null` where the static selector matches nothing;
//! - `options()`: a fresh transform `options` object;
//! - `finish(index, root, result, staged)`: the transform's awaited result
//!   and its staged changes as the `FileOutcome` the bridge would return.
//!
//! The only binding-specific code here is reading the transform's return
//! value ([`read_return`]), which mirrors the QuickJS facade's
//! `extract_transform_output` / `JsValue::from_js`; every rule after that is
//! shared Rust.

use std::sync::Arc;

use codemod_sandbox::sandbox::engine::{
    classify_transform_result, missing_output_error, output_json_error, CodemodOutput,
    ExecutionResult, FileChange, ModificationCheck, ReturnedContent,
};
use codemod_sandbox::sg_core::CoreRoot;
use jssg_batch::{BatchConfig, BatchPlan, SemanticSupport};
use napi::bindgen_prelude::*;
use napi::{JsValue, ValueType};
use napi_derive::napi;

use crate::SgRoot;

/// The changes one transform call staged through `write()` and
/// `jssgTransform()` (opaque to JavaScript; consumed by `JssgBatch.finish`).
#[napi]
pub struct StagedChanges {
    pub(crate) changes: Vec<FileChange>,
}

/// One planned JSSG batch.
#[napi]
pub struct JssgBatch {
    plan: Arc<BatchPlan>,
}

#[napi]
impl JssgBatch {
    /// Plan a batch from its JSON config (`jssg_batch::BatchConfig`) and the
    /// root-relative paths of its files; throws the planning error.
    #[napi(constructor)]
    pub fn new(config: String, paths: Vec<String>) -> Result<Self> {
        let config: BatchConfig = serde_json::from_str(&config)
            .map_err(|error| Error::from_reason(format!("invalid JSSG batch: {error}")))?;
        let plan = BatchPlan::for_paths(
            &config.batch(),
            paths.iter().map(String::as_str),
            SemanticSupport::Unsupported("node"),
        )
        .map_err(Error::from_reason)?;
        Ok(Self {
            plan: Arc::new(plan),
        })
    }

    /// Virtual module name of the artifact (`<name>.jssg.js`).
    #[napi(getter)]
    pub fn module(&self) -> String {
        self.plan.module.clone()
    }

    /// The canonical target root.
    #[napi(getter, js_name = "targetRoot")]
    pub fn target_root(&self) -> String {
        self.plan.root.to_string_lossy().into_owned()
    }

    /// `options.params`: `{ input }` when the operation has an input, else `{}`.
    #[napi(getter)]
    pub fn params(&self) -> serde_json::Value {
        params(&self.plan)
    }

    /// A fresh transform `options` object, as the sandbox builds it:
    /// `{ params, language, matches: undefined, matrixValues: undefined,
    /// dryRun: false, targetDir }`.
    #[napi(
        ts_return_type = "{ params: Record<string, unknown>; language: string; matches: undefined; matrixValues: undefined; dryRun: false; targetDir: string }"
    )]
    pub fn options<'env>(&self, env: &'env Env) -> Result<Object<'env>> {
        let mut options = Object::new(env)?;
        options.set("params", params(&self.plan))?;
        options.set("language", self.plan.language_name())?;
        options.set("matches", ())?;
        options.set("matrixValues", ())?;
        options.set("dryRun", false)?;
        options.set("targetDir", self.target_root())?;
        Ok(options)
    }

    /// Parse files `start ..< start + contents.length` off the JS thread (in
    /// parallel); `null` where the static selector matches nothing.
    #[napi(ts_return_type = "Promise<Array<SgRoot | null>>")]
    pub fn parse(&self, start: u32, contents: Vec<String>) -> AsyncTask<ParseChunk> {
        AsyncTask::new(ParseChunk {
            plan: Arc::clone(&self.plan),
            start: start as usize,
            contents,
        })
    }

    /// File `index`'s awaited transform `result` and `staged` changes as its
    /// `FileOutcome`; throws `JSSG failed for '<path>': ...`.
    #[napi(ts_return_type = "FileOutcome")]
    pub fn finish(
        &self,
        index: u32,
        root: &SgRoot,
        result: Unknown<'_>,
        staged: &mut StagedChanges,
    ) -> Result<serde_json::Value> {
        let secondary = std::mem::take(&mut staged.changes);
        let output = transform_output(&root.core, result, secondary);
        let outcome = self
            .plan
            .finish(index as usize, output)
            .map_err(Error::from_reason)?;
        serde_json::to_value(outcome).map_err(|error| Error::from_reason(error.to_string()))
    }
}

fn params(plan: &BatchPlan) -> serde_json::Value {
    serde_json::Value::Object(
        plan.params
            .iter()
            .flatten()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

/// One chunk of a batch parsed on worker threads.
pub struct ParseChunk {
    plan: Arc<BatchPlan>,
    start: usize,
    contents: Vec<String>,
}

impl Task for ParseChunk {
    type Output = Vec<Option<CoreRoot>>;
    type JsValue = Vec<Option<SgRoot>>;

    fn compute(&mut self) -> Result<Self::Output> {
        let len = self.contents.len();
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(len.div_ceil(4))
            .max(1);
        let chunk = len.div_ceil(threads).max(1);
        let (plan, start, contents) = (&*self.plan, self.start, &self.contents);
        let parts: Vec<std::result::Result<Vec<Option<CoreRoot>>, String>> =
            std::thread::scope(|scope| {
                let handles: Vec<_> = (0..len)
                    .step_by(chunk)
                    .map(|from| {
                        scope.spawn(move || {
                            (from..(from + chunk).min(len))
                                .map(|offset| plan.parse(start + offset, &contents[offset]))
                                .collect::<std::result::Result<Vec<_>, _>>()
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|handle| {
                        handle
                            .join()
                            .unwrap_or_else(|_| Err("parser thread panicked".to_string()))
                    })
                    .collect()
            });
        let mut roots = Vec::with_capacity(len);
        for part in parts {
            roots.extend(part.map_err(Error::from_reason)?);
        }
        Ok(roots)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output
            .into_iter()
            .map(|root| root.map(SgRoot::from_core))
            .collect())
    }
}

/// The transform's awaited return value plus its staged changes as a
/// [`CodemodOutput`], with the sandbox's rules.
fn transform_output(
    root: &CoreRoot,
    result: Unknown<'_>,
    secondary: Vec<FileChange>,
) -> std::result::Result<CodemodOutput, String> {
    let (content, output) = read_return(result).map_err(|error| error.to_string())?;
    let original = root.source();
    let primary = classify_transform_result(
        content,
        root,
        ModificationCheck::StringEquality {
            original_content: &original,
        },
    )
    .map_err(|error| error.to_string())?;
    Ok(CodemodOutput {
        primary,
        secondary,
        output,
    })
}

/// `jssgTransform()`'s inner result for the file at `path` parsed as `root`:
/// staged as a secondary change; the new content, or `None` when unmodified.
pub(crate) fn stage_secondary(
    path: String,
    root: &CoreRoot,
    result: Unknown<'_>,
) -> std::result::Result<(Option<String>, FileChange), String> {
    if matches!(
        result.get_type(),
        Ok(ValueType::Object | ValueType::Function)
    ) {
        return Err(codemod_sandbox::sandbox::engine::SECONDARY_STRUCTURED_RESULT.to_string());
    }
    let content = returned_content(result).map_err(|error| error.to_string())?;
    let original = root.source();
    let result = classify_transform_result(
        content,
        root,
        ModificationCheck::StringEquality {
            original_content: &original,
        },
    )
    .map_err(|error| format!("Transform result error: {error}"))?;
    let modified = match &result {
        ExecutionResult::Modified(modified) => Some(modified.content.clone()),
        _ => None,
    };
    Ok((
        modified,
        FileChange {
            path: path.into(),
            result,
        },
    ))
}

// ---------------------------------------------------------------------------
// Reading a transform's return value (mirrors the QuickJS facade)
// ---------------------------------------------------------------------------

/// `extract_transform_output`: an object (anything of JS object type) is
/// `{ content, output }` and must carry `output`; anything else is the
/// content itself.
fn read_return(
    value: Unknown<'_>,
) -> std::result::Result<
    (ReturnedContent, Option<serde_json::Value>),
    codemod_sandbox::sandbox::errors::ExecutionError,
> {
    let internal = |error: Error| output_json_error(error.reason);
    match value.get_type().map_err(internal)? {
        ValueType::Object | ValueType::Function => {
            let object: Object = unsafe { value.cast() }.map_err(internal)?;
            let Some(output) = object.get::<Unknown>("output").map_err(internal)? else {
                return Err(missing_output_error());
            };
            let output = to_json(output).map_err(internal)?;
            let content = match object.get::<Unknown>("content").map_err(internal)? {
                Some(content) => returned_content(content).map_err(internal)?,
                None => ReturnedContent::Nullish,
            };
            Ok((content, Some(output)))
        }
        _ => Ok((returned_content(value).map_err(internal)?, None)),
    }
}

fn returned_content(value: Unknown<'_>) -> Result<ReturnedContent> {
    Ok(match value.get_type()? {
        ValueType::String => ReturnedContent::Text(unsafe { value.cast::<String>() }?),
        ValueType::Null | ValueType::Undefined => ReturnedContent::Nullish,
        _ => ReturnedContent::Other(quickjs_type_name(&value)?.to_string()),
    })
}

/// QuickJS's name for the value's type (`rquickjs::Type`), which the
/// sandbox puts in its "must return a string or null" error.
fn quickjs_type_name(value: &Unknown<'_>) -> Result<&'static str> {
    Ok(match value.get_type()? {
        ValueType::Undefined => "undefined",
        ValueType::Null => "null",
        ValueType::Boolean => "bool",
        ValueType::Number => {
            if as_int(unsafe { value.cast::<f64>() }?).is_some() {
                "int"
            } else {
                "float"
            }
        }
        ValueType::String => "string",
        ValueType::Symbol => "symbol",
        ValueType::BigInt => "big_int",
        ValueType::Function => "function",
        ValueType::Object if value.is_array()? => "array",
        ValueType::Object if value.is_promise()? => "promise",
        ValueType::Object if value.is_error()? => "exception",
        ValueType::Object | ValueType::External => "object",
        _ => "unknown",
    })
}

/// A number QuickJS would hold as an `int` (an `i32`, not `-0`).
fn as_int(number: f64) -> Option<i32> {
    let fits = number.fract() == 0.0
        && number >= f64::from(i32::MIN)
        && number <= f64::from(i32::MAX)
        && !(number == 0.0 && number.is_sign_negative());
    fits.then_some(number as i32)
}

/// `JsValue::from_js` of the QuickJS facade: undefined/null, functions,
/// symbols, promises, and errors become `null`; arrays keep every element;
/// objects keep their own enumerable string keys; numbers keep their value.
fn to_json(value: Unknown<'_>) -> Result<serde_json::Value> {
    use serde_json::Value as Json;
    Ok(match value.get_type()? {
        ValueType::Boolean => Json::Bool(unsafe { value.cast::<bool>() }?),
        ValueType::Number => {
            let number = unsafe { value.cast::<f64>() }?;
            match as_int(number) {
                Some(int) => Json::Number(int.into()),
                None => serde_json::Number::from_f64(number)
                    .map(Json::Number)
                    .unwrap_or(Json::Null),
            }
        }
        ValueType::BigInt => {
            let (int, _lossless) = unsafe { value.cast::<BigInt>() }?.get_i64();
            Json::Number(int.into())
        }
        ValueType::String => Json::String(unsafe { value.cast::<String>() }?),
        ValueType::Object if value.is_array()? => {
            let array: Array = unsafe { value.cast() }?;
            let mut values = Vec::with_capacity(array.len() as usize);
            for index in 0..array.len() {
                if let Some(element) = array.get::<Unknown>(index)? {
                    values.push(to_json(element)?);
                } else {
                    values.push(Json::Null);
                }
            }
            Json::Array(values)
        }
        ValueType::Object if value.is_promise()? || value.is_error()? => Json::Null,
        ValueType::Object => {
            let object: Object = unsafe { value.cast() }?;
            // Own enumerable string keys, like `rquickjs::Object::keys`.
            let env = value.value().env;
            let mut names = std::ptr::null_mut();
            napi::check_status!(unsafe {
                napi::sys::napi_get_all_property_names(
                    env,
                    value.raw(),
                    napi::sys::KeyCollectionMode::own_only,
                    napi::sys::KeyFilter::enumerable | napi::sys::KeyFilter::skip_symbols,
                    napi::sys::KeyConversion::numbers_to_strings,
                    &mut names,
                )
            })?;
            let keys: Array = unsafe { Array::from_napi_value(env, names) }?;
            let mut map = serde_json::Map::new();
            for index in 0..keys.len() {
                let Some(key) = keys.get::<String>(index)? else {
                    continue;
                };
                let property = object.get::<Unknown>(&key)?;
                map.insert(
                    key,
                    match property {
                        Some(property) => to_json(property)?,
                        None => Json::Null,
                    },
                );
            }
            Json::Object(map)
        }
        _ => Json::Null,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::path::Path;

    fn plan(
        root: &Path,
        paths: &[&str],
        selector: Option<&str>,
    ) -> std::result::Result<BatchPlan, String> {
        let source = "export default () => null;";
        let mut config = serde_json::json!({
            "transform": { "name": "t", "hash": format!("{:x}", Sha256::digest(source)) },
            "source": source,
            "language": "typescript",
            "targetRoot": root,
        });
        if let Some(pattern) = selector {
            config["selector"] = serde_json::json!({ "rule": { "pattern": pattern } });
        }
        let config: BatchConfig = serde_json::from_value(config).unwrap();
        BatchPlan::for_paths(
            &config.batch(),
            paths.iter().copied(),
            SemanticSupport::Unsupported("node"),
        )
    }

    #[test]
    fn parses_in_parallel_in_order_and_applies_the_selector() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let files: Vec<(String, String)> = (0..37)
            .map(|i| {
                let content = if i % 3 == 0 {
                    format!("oldApi({i});\n")
                } else {
                    format!("other({i});\n")
                };
                (format!("dir/f{i}.ts"), content)
            })
            .collect();
        std::fs::create_dir_all(root.join("dir")).unwrap();
        for (path, content) in &files {
            std::fs::write(root.join(path), content).unwrap();
        }
        let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        let contents: Vec<String> = files.iter().map(|(_, c)| c.clone()).collect();
        for selector in [Some("oldApi($A)"), None] {
            let plan = Arc::new(plan(&root, &paths, selector).unwrap());
            // Two chunks, the second starting mid-batch.
            let mut roots = Vec::new();
            for (start, end) in [(0, 20), (20, files.len())] {
                let mut chunk = ParseChunk {
                    plan: Arc::clone(&plan),
                    start,
                    contents: contents[start..end].to_vec(),
                };
                roots.extend(chunk.compute().unwrap());
            }
            assert_eq!(roots.len(), files.len());
            for (i, parsed) in roots.iter().enumerate() {
                assert_eq!(
                    parsed.is_some(),
                    selector.is_none() || i % 3 == 0,
                    "file {i}"
                );
                if let Some(parsed) = parsed {
                    assert_eq!(
                        parsed.relative_filename(),
                        Some(format!("dir/f{i}.ts").as_str())
                    );
                    assert_eq!(parsed.source(), files[i].1);
                }
            }
        }
    }

    #[test]
    fn refuses_paths_outside_the_root_and_semantic_analysis() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for path in ["../x.ts", "/etc/passwd", "a/../../x.ts", ""] {
            let error = plan(&root, &[path], None)
                .err()
                .unwrap_or_else(|| panic!("{path:?} was accepted"));
            assert!(
                error.contains("safe relative path") || error.contains("must not be empty"),
                "{error}"
            );
        }
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("x.ts"), "x").unwrap();
            std::os::unix::fs::symlink(outside.path(), root.join("link")).unwrap();
            let error = plan(&root, &["link/x.ts"], None).err().unwrap();
            assert!(error.contains("escapes the target root"), "{error}");
        }
    }

    #[test]
    fn type_names_match_quickjs() {
        assert_eq!(as_int(42.0), Some(42));
        assert_eq!(as_int(-0.0), None);
        assert_eq!(as_int(1.5), None);
        assert_eq!(as_int(f64::from(i32::MAX) + 1.0), None);
    }
}
