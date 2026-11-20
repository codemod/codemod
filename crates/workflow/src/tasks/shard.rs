//! `shard({ method, target, files, filter, previous })`: split files into
//! shards, with the YAML engine's built-in algorithms or a shard function.
//! `files` selects the eligible files under `target`, and `filter`, a
//! per-file callback, keeps those it returns `true` for. The shard function
//! and the filter are extracted by the build and run in isolates, as
//! callbacks do. Passing the previous result as `previous` keeps assignments
//! stable.

use butterflow_core::shard::{evaluate_builtin_shards, ResolvedBuiltinShardMethod, ShardResult};
use butterflow_models::step::{BuiltinShardType, ShardMethod, UseShard};
use serde::Deserialize;
use serde_json::{json, Value};

use super::for_each_file::{self, Selector};
use super::{function, TaskContext, TaskError, TaskOutcome, TaskResult};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    method: Method,
    target: Option<String>,
    files: Option<Files>,
    filter: Option<Extracted>,
    previous: Option<Vec<Value>>,
}

#[derive(Deserialize, Default)]
struct Files {
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    language: Option<String>,
}

/// A function the build extracted, and the values it captured.
#[derive(Deserialize)]
struct Extracted {
    callback: String,
    #[serde(default)]
    captures: Value,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Method {
    Function {
        function: Extracted,
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
    let eligible = eligible(task, &request)?;

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
                file_pattern: None,
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
        Method::Function { function } => {
            let files = eligible.ok_or_else(|| {
                TaskError::Failed(
                    "a shard function needs files to choose from: give shard `files` (a glob or a selector), or a `filter`".into(),
                )
            })?;
            let input = json!({
                "files": files,
                "targetDir": target.to_string_lossy(),
                "previousShards": request.previous.clone().unwrap_or_default(),
            });
            let (output, _) = function::call(
                task,
                &function.callback,
                &function.captures,
                Some(&json!([input])),
            )
            .map_err(|e| TaskError::Failed(format!("the shard function failed: {e}")))?;
            serde_json::from_value(output).map_err(|e| {
                TaskError::Failed(format!(
                    "the shard function returned an invalid result: {e}"
                ))
            })?
        }
    };
    task.log(format!("{} shards", shards.len()));
    Ok(TaskOutcome::new(
        serde_json::to_value(shards).map_err(|e| e.to_string())?,
    ))
}

/// The files shards are made of, relative to the run's target: those
/// `files` selects under `target` (the package's languages without one),
/// then those `filter` keeps. `None` when neither is given: built-in
/// methods then take every file under `target`.
fn eligible(task: &TaskContext<'_>, request: &Request) -> Result<Option<Vec<String>>, TaskError> {
    if request.files.is_none() && request.filter.is_none() {
        return Ok(None);
    }
    let run = task.run;
    let base_dir = request
        .target
        .as_deref()
        .map(|target| target.trim_start_matches("./").trim_end_matches('/'))
        .filter(|target| !target.is_empty() && *target != ".");
    let files = request
        .files
        .as_ref()
        .map_or_else(Files::default, |files| Files {
            include: files.include.clone(),
            exclude: files.exclude.clone(),
            language: files.language.clone(),
        });
    let mut selector = Selector {
        include: files.include,
        exclude: files.exclude,
        language: files.language,
        ..Selector::default()
    };
    let default_languages = if selector.include.is_empty() && selector.language.is_none() {
        for_each_file::package_languages(&run.package_dir)?
    } else {
        Vec::new()
    };
    if selector.include.is_empty() && selector.language.is_none() && default_languages.is_empty() {
        return Err(TaskError::Failed(
            "shard needs to know which files to choose from: give it `files` (a glob or { include, language }), or list the package's languages under `targets.languages` in codemod.yaml".into(),
        ));
    }
    if selector.language.is_none() && default_languages.len() == 1 {
        selector.language = default_languages.first().cloned();
    }
    let base = match base_dir {
        Some(dir) => run.target.join(dir),
        None => run.target.clone(),
    };
    let under_target = |path: String| match base_dir {
        Some(dir) => format!("{dir}/{path}"),
        None => path,
    };
    let selected = for_each_file::select(&base, &selector, &default_languages)?;
    let Some(filter) = &request.filter else {
        // Only text files: the ones a `forEachFile` callback would get.
        return Ok(Some(
            for_each_file::read_selected(&base, selected, None)
                .into_iter()
                .map(|(path, _)| under_target(path))
                .collect(),
        ));
    };
    let selected: Vec<String> = selected.into_iter().map(under_target).collect();
    let outputs = for_each_file::callback_outputs(
        task,
        &filter.callback,
        &filter.captures,
        &selector,
        &selected,
    )?;
    let mut kept = Vec::new();
    // What the YAML compatibility filter reports for a file its transform's
    // selector matched but would not change: kept only when no file would.
    let mut matched = Vec::new();
    let mut failures = Vec::new();
    for (path, output) in outputs {
        match output {
            Ok(Value::Bool(true)) => kept.push(path),
            Ok(Value::String(text)) if text == "matched" => matched.push(path),
            Ok(_) => {}
            Err(error) => failures.push((path, error)),
        }
    }
    task.log_failures(&failures);
    Ok(Some(if kept.is_empty() { matched } else { kept }))
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
