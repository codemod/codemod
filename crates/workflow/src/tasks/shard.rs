//! `shard({ method, target, filePattern, jssg, previous })`: split files into
//! shards, with the YAML engine's built-in algorithms or a custom function.
//! Passing the previous result as `previous` keeps assignments stable.

use std::path::Path;
use std::sync::Arc;

use butterflow_core::shard::{
    collect_files_with_pattern, evaluate_builtin_shards, ResolvedBuiltinShardMethod, ShardResult,
};
use butterflow_models::step::{BuiltinShardType, ShardMethod, UseShard};
use codemod_sandbox::sandbox::engine::execution_engine::{
    execute_shard_function_with_quickjs, ShardFunctionOptions,
};
use codemod_sandbox::sandbox::resolvers::OxcResolver;
use codemod_sandbox::utils::project_discovery::find_tsconfig;
use serde::Deserialize;
use serde_json::{json, Value};

use super::changes::{bundle_path, relative};
use super::{block_on, jssg, TaskContext, TaskError, TaskOutcome, TaskResult};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    method: Method,
    target: Option<String>,
    file_pattern: Option<String>,
    /// Only files this transform changes (or its selector matches).
    jssg: Option<jssg::Request>,
    previous: Option<Vec<Value>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Method {
    Function {
        function: String,
    },
    #[serde(rename_all = "camelCase")]
    Builtin {
        r#type: BuiltinShardType,
        max_files_per_shard: Value,
        min_shard_size: Option<Value>,
    },
}

pub(crate) fn run(task: &TaskContext<'_>, content: Value) -> TaskResult {
    if task.run.services.skip_shard_steps {
        task.log("skipped: shard steps are disabled for this run");
        return Ok(TaskOutcome::new(json!([])));
    }
    let request: Request = serde_json::from_value(content)
        .map_err(|e| TaskError::Failed(format!("invalid shard options: {e}")))?;
    let target = &task.run.target;

    let eligible = match &request.jssg {
        Some(transform) => Some(prescan(task, transform, request.target.as_deref())?),
        None => None,
    };

    let shards: Vec<ShardResult> = match &request.method {
        Method::Builtin {
            r#type,
            max_files_per_shard,
            min_shard_size,
        } => {
            let resolved = ResolvedBuiltinShardMethod {
                r#type: *r#type,
                max_files_per_shard: as_count(max_files_per_shard, "maxFilesPerShard")?,
                min_shard_size: min_shard_size
                    .as_ref()
                    .map(|value| as_count(value, "minShardSize"))
                    .transpose()?,
            };
            let config = UseShard {
                method: ShardMethod::Builtin(butterflow_models::step::BuiltinShardMethod {
                    r#type: *r#type,
                    max_files_per_shard: max_files_per_shard.clone(),
                    min_shard_size: min_shard_size.clone(),
                }),
                target: request.target.clone(),
                output_state: String::new(),
                file_pattern: request.file_pattern.clone(),
                js_ast_grep: None,
            };
            evaluate_builtin_shards(
                &config,
                target,
                eligible.as_deref(),
                request.previous.as_ref(),
                &resolved,
            )
            .map_err(TaskError::Failed)?
        }
        Method::Function { function } => custom(task, function, &request, eligible.as_deref())?,
    };
    task.log(format!("{} shards", shards.len()));
    Ok(TaskOutcome::new(
        serde_json::to_value(shards).map_err(|e| e.to_string())?,
    ))
}

fn as_count(value: &Value, name: &str) -> Result<usize, TaskError> {
    match value {
        Value::Number(number) => number.as_u64().map(|n| n as usize),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
    .filter(|n| *n > 0)
    .ok_or_else(|| TaskError::Failed(format!("{name} must be a positive number, not {value}")))
}

/// Files the transform would change, or failing that, the files its
/// selector matches; relative to the target.
fn prescan(
    task: &TaskContext<'_>,
    transform: &jssg::Request,
    shard_target: Option<&str>,
) -> Result<Vec<String>, TaskError> {
    let mut request = transform.clone();
    if let Some(shard_target) = shard_target {
        request.base_path = Some(shard_target.to_string());
    }
    let outcome = jssg::execute(task, &request, true)?;
    let target = &task.run.target;
    let to_relative = |path: &Path| relative(target, path);
    let modified: Vec<String> = outcome
        .changes
        .iter()
        .filter_map(|change| to_relative(&change.path))
        .collect();
    if !modified.is_empty() {
        return Ok(modified);
    }
    Ok(outcome
        .matched
        .iter()
        .filter_map(|path| to_relative(path))
        .collect())
}

fn custom(
    task: &TaskContext<'_>,
    function: &str,
    request: &Request,
    eligible: Option<&[String]>,
) -> Result<Vec<ShardResult>, TaskError> {
    let target = &task.run.target;
    let script = bundle_path(task, function, "shard function")?;
    if !script.is_file() {
        return Err(TaskError::Failed(format!(
            "shard function '{}' does not exist",
            script.display()
        )));
    }
    let files: Vec<String> = match (eligible, &request.file_pattern) {
        (Some(files), _) => files.to_vec(),
        (None, Some(pattern)) => {
            let base = target.join(request.target.as_deref().unwrap_or("."));
            collect_files_with_pattern(&base, pattern)
                .map_err(|e| TaskError::Failed(format!("cannot collect files: {e}")))?
                .iter()
                .filter_map(|file| relative(target, file))
                .collect()
        }
        (None, None) => {
            return Err(TaskError::Failed(
                "a shard function needs 'filePattern' or 'jssg' to find files".into(),
            ))
        }
    };
    let script_dir = script.parent().unwrap_or(Path::new(".")).to_path_buf();
    let resolver = Arc::new(
        OxcResolver::new(script_dir.clone(), find_tsconfig(&script_dir))
            .map_err(|e| TaskError::Failed(format!("cannot resolve modules: {e}")))?,
    );
    let input = json!({
        "files": files,
        "targetDir": target.to_string_lossy(),
        "previousShards": request.previous.clone().unwrap_or_default(),
    });
    let output = block_on(execute_shard_function_with_quickjs(ShardFunctionOptions {
        script_path: &script,
        resolver,
        input,
        capabilities: Some(task.run.services.capabilities.clone()),
        target_directory: Some(target),
    }))
    .map_err(|e| TaskError::Failed(format!("the shard function failed: {e}")))?;
    serde_json::from_value(output).map_err(|e| {
        TaskError::Failed(format!(
            "the shard function returned an invalid result: {e}"
        ))
    })
}
