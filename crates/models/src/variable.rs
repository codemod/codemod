use evalexpr::{
    eval_with_context, Context, ContextWithMutableVariables, HashMapContext, Value as EvalValue,
};
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Matches a full `${{ expression }}` template string (anchored).
static EXPR_ONLY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\$\{\{\s*([^}]+?)\s*\}\}$").expect("valid expr-only regex"));

/// Matches all `${{ expression }}` template patterns (unanchored).
static EXPR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\{\{\s*([^}]+?)\s*\}\}").expect("valid expr regex"));

/// Matches `task.<var_name>` references inside expressions.
static TASK_VAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"task\.([a-zA-Z_][a-zA-Z0-9_]*)").expect("valid task var regex"));

use crate::error::Error;
use crate::Result;

fn convert_value_to_eval_value(value: &Value) -> EvalValue {
    match value {
        Value::String(s) => {
            if let Ok(num) = s.parse::<f64>() {
                EvalValue::Float(num)
            } else {
                EvalValue::String(s.clone())
            }
        }
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                EvalValue::Int(i)
            } else if let Some(f) = n.as_f64() {
                EvalValue::Float(f)
            } else {
                EvalValue::String(n.to_string())
            }
        }
        Value::Bool(b) => EvalValue::Boolean(*b),
        Value::Null => EvalValue::Empty,
        _ => EvalValue::String(value.to_string()),
    }
}

pub fn resolve_state_path<'a>(state: &'a HashMap<String, Value>, path: &str) -> Result<&'a Value> {
    let parts: Vec<&str> = path.split('.').collect();

    if parts.is_empty() {
        return Err(Error::VariableResolution("Empty path".to_string()));
    }

    let root_key = parts[0];
    let mut current_value = state
        .get(root_key)
        .ok_or_else(|| Error::VariableResolution(format!("State key not found: {root_key}")))?;

    for part in &parts[1..] {
        current_value = match current_value {
            Value::Object(obj) => obj.get(*part).ok_or_else(|| {
                Error::VariableResolution(format!("Object key not found: {part}"))
            })?,
            Value::Array(arr) => {
                let index: usize = part.parse().map_err(|_| {
                    Error::VariableResolution(format!("Invalid array index: {part}"))
                })?;
                arr.get(index).ok_or_else(|| {
                    Error::VariableResolution(format!("Array index out of bounds: {index}"))
                })?
            }
            _ => {
                return Err(Error::VariableResolution(format!(
                    "Cannot traverse path '{part}' - value is not an object or array"
                )));
            }
        };
    }

    Ok(current_value)
}

/// Extra context for task-level template variables (`task.id`, `task.signature`, etc.).
///
/// In addition to the built-in `id` and `signature` fields, arbitrary extra
/// variables can be supplied via `CODEMOD_TASK_*` environment variables.
/// For example, `CODEMOD_TASK_JIRA_TITLE=Fix bug` becomes `task.jira_title`.
#[derive(Debug, Clone)]
pub struct TaskExpressionContext {
    /// Raw task ID (e.g. the UUID string from `CODEMOD_TASK_ID`)
    pub id: String,
    /// Short deterministic hash derived from the task ID (first 8 hex chars of SHA-256)
    pub signature: String,
    /// Additional task variables derived from `CODEMOD_TASK_*` environment variables.
    /// Keys are lowercase with the `CODEMOD_TASK_` prefix stripped
    /// (e.g. `CODEMOD_TASK_JIRA_TITLE` → `jira_title`).
    pub extra: std::collections::HashMap<String, String>,
}

pub fn resolve_expressions(
    expression: &str,
    params: &HashMap<String, Value>,
    state: &HashMap<String, Value>,
    matrix_values: Option<&HashMap<String, Value>>,
    steps: Option<&HashMap<String, HashMap<String, String>>>,
    task_context: Option<&TaskExpressionContext>,
) -> Result<EvalValue> {
    let mut context = HashMapContext::new();

    for (key, value) in params {
        let context_key = format!("params.{}", key);
        let eval_value = convert_value_to_eval_value(value);
        context.set_value(context_key, eval_value)?;
    }

    for (key, value) in state {
        let context_key = format!("state.{}", key);
        let eval_value = convert_value_to_eval_value(value);
        context.set_value(context_key, eval_value)?;
    }

    if let Some(matrix) = matrix_values {
        for (key, value) in matrix {
            let context_key = format!("matrix.{}", key);
            let eval_value = convert_value_to_eval_value(value);
            context.set_value(context_key, eval_value)?;
        }
    }

    if let Some(steps_map) = steps {
        for (step_id, outputs) in steps_map {
            for (output_name, output_value) in outputs {
                let context_key = format!("steps.{}.outputs.{}", step_id, output_name);
                if let Ok(num) = output_value.parse::<f64>() {
                    context.set_value(context_key, EvalValue::Float(num))?;
                } else {
                    context.set_value(context_key, EvalValue::String(output_value.clone()))?;
                }
            }
        }
    }

    if let Some(task_ctx) = task_context {
        context.set_value(
            "task.id".to_string(),
            EvalValue::String(task_ctx.id.clone()),
        )?;
        context.set_value(
            "task.signature".to_string(),
            EvalValue::String(task_ctx.signature.clone()),
        )?;
        for (key, value) in &task_ctx.extra {
            let context_key = format!("task.{}", key);
            context.set_value(context_key, EvalValue::String(value.clone()))?;
        }
    }

    // Pre-populate any `task.*` identifiers referenced in the expression that
    // are not already in the context with an empty string so that missing
    // CODEMOD_TASK_* env vars resolve gracefully instead of erroring.
    for cap in TASK_VAR_RE.captures_iter(expression) {
        let var_name = format!("task.{}", &cap[1]);
        if context.get_value(&var_name).is_none() {
            context.set_value(var_name, EvalValue::String(String::new()))?;
        }
    }

    eval_with_context(expression, &context).map_err(Error::ExpressionEvaluation)
}

/// Evaluate if a condition string resolves to a truthy value
/// First tries to resolve as an expression with operators, falls back to simple variable resolution
pub fn evaluate_condition(
    condition: &str,
    params: &HashMap<String, Value>,
    state: &HashMap<String, Value>,
    matrix_values: Option<&HashMap<String, Value>>,
    steps: Option<&HashMap<String, HashMap<String, String>>>,
    task_context: Option<&TaskExpressionContext>,
) -> Result<bool> {
    let result = resolve_expressions(condition, params, state, matrix_values, steps, task_context)?;
    match result {
        EvalValue::Boolean(v) => Ok(v),
        EvalValue::Empty => Ok(false),
        EvalValue::Float(v) => Ok(v != 0.0),
        EvalValue::Int(v) => Ok(v != 0),
        EvalValue::String(v) => Ok(!v.is_empty() && v != "false"),
        EvalValue::Tuple(v) => Ok(!v.is_empty()),
    }
}

/// Resolve template strings with expressions in ${{ }} syntax
/// Example: "Hello ${{ params.name }} ${{ 1 + 2 }}" -> "Hello John Doe 3"
pub fn resolve_string_with_expression(
    template: &str,
    params: &HashMap<String, Value>,
    state: &HashMap<String, Value>,
    matrix_values: Option<&HashMap<String, Value>>,
    steps: Option<&HashMap<String, HashMap<String, String>>>,
    task_context: Option<&TaskExpressionContext>,
) -> Result<String> {
    let mut result = template.to_string();

    for captures in EXPR_RE.captures_iter(template) {
        let full_match = captures.get(0).unwrap().as_str();
        let expression = captures.get(1).unwrap().as_str().trim();

        let eval_result = resolve_expressions(
            expression,
            params,
            state,
            matrix_values,
            steps,
            task_context,
        )?;

        let replacement = match eval_result {
            EvalValue::String(s) => s,
            EvalValue::Int(i) => i.to_string(),
            EvalValue::Float(f) => f.to_string(),
            EvalValue::Boolean(b) => b.to_string(),
            EvalValue::Empty => "".to_string(),
            EvalValue::Tuple(t) => format!("{:?}", t),
        };

        result = result.replace(full_match, &replacement);
    }

    Ok(result)
}

/// Resolve a list of strings that may contain `${{ }}` template expressions.
///
/// Each element is resolved via [`resolve_string_with_expression`]. If an
/// element is *solely* a `${{ expr }}` expression and the referenced value is a
/// JSON array (e.g. from state or params), the array items are expanded inline
/// into the result list. This enables patterns like:
///
/// ```yaml
/// include:
///   - "${{ state.files }}"        # expands JSON array ["a.js","b.js"] → two entries
///   - "**/*.test.ts"              # kept as-is
///   - "${{ params.extra_glob }}"  # scalar string kept as single entry
/// ```
pub fn resolve_string_list(
    items: &[String],
    params: &HashMap<String, Value>,
    state: &HashMap<String, Value>,
    matrix_values: Option<&HashMap<String, Value>>,
    steps: Option<&HashMap<String, HashMap<String, String>>>,
    task_context: Option<&TaskExpressionContext>,
) -> Result<Vec<String>> {
    let mut result = Vec::new();

    for item in items {
        // If the entire item is a single expression, check whether the
        // underlying value is an array so we can expand it.
        if let Some(caps) = EXPR_ONLY_RE.captures(item) {
            let expression = caps.get(1).unwrap().as_str().trim();

            // Try to look up the raw JSON value from state / params / matrix
            // so we can detect arrays before evalexpr stringifies them.
            if let Some(array_items) = lookup_json_array(expression, params, state, matrix_values) {
                for v in array_items {
                    result.push(v);
                }
                continue;
            }
        }

        // Default: resolve as a template string (scalar).
        let resolved = resolve_string_with_expression(
            item,
            params,
            state,
            matrix_values,
            steps,
            task_context,
        )?;
        if !resolved.is_empty() {
            result.push(resolved);
        }
    }

    Ok(result)
}

/// Try to look up a dotted identifier (e.g. `state.files`, `params.globs`,
/// `matrix.batch`) as a raw JSON value and, if it is an array of strings,
/// return the items. Returns `None` for non-array values or unknown paths.
fn lookup_json_array(
    expression: &str,
    params: &HashMap<String, Value>,
    state: &HashMap<String, Value>,
    matrix_values: Option<&HashMap<String, Value>>,
) -> Option<Vec<String>> {
    let (scope, key) = expression.split_once('.')?;
    let map: &HashMap<String, Value> = match scope {
        "state" => state,
        "params" => params,
        "matrix" => matrix_values?,
        _ => return None,
    };

    let value = map.get(key)?;

    value_to_string_vec(value)
}

/// Convert a JSON value to a `Vec<String>` if it is an array of strings, or a
/// JSON-encoded string containing an array of strings. Returns `None` otherwise.
pub fn value_to_string_vec(value: &Value) -> Option<Vec<String>> {
    match value {
        Value::Array(arr) => {
            let strings: Vec<String> = arr
                .iter()
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect();
            Some(strings)
        }
        // Handle JSON-encoded array strings (e.g. from CODEMOD_STATE_* env vars)
        Value::String(s) => {
            if let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(s) {
                let strings: Vec<String> = arr
                    .iter()
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .collect();
                Some(strings)
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn create_test_params() -> HashMap<String, Value> {
        let mut params = HashMap::new();
        params.insert("environment".to_string(), json!("production"));
        params.insert("skip_tests".to_string(), json!("false"));
        params.insert("version".to_string(), json!("2.1"));
        params.insert("max_attempts".to_string(), json!("5"));
        params.insert("empty_param".to_string(), json!(""));
        params
    }

    fn create_test_state() -> HashMap<String, Value> {
        let mut state = HashMap::new();
        state.insert("deploy_ready".to_string(), json!(true));
        state.insert("retry_count".to_string(), json!(3));
        state.insert("last_status".to_string(), json!("success"));
        state.insert("config_loaded".to_string(), json!(false));
        state.insert("null_value".to_string(), json!(null));
        state
    }

    fn create_test_matrix() -> HashMap<String, Value> {
        let mut matrix = HashMap::new();
        matrix.insert("os".to_string(), json!("linux"));
        matrix.insert("node_version".to_string(), json!(18)); // Store as number
        matrix.insert("parallel".to_string(), json!(true));
        matrix
    }

    #[test]
    fn test_resolve_expressions_equality_operators() {
        let params = create_test_params();
        let state = create_test_state();
        let matrix = create_test_matrix();

        // String equality - should return boolean true
        assert_eq!(
            resolve_expressions(
                r#"params.environment == "production""#,
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions(
                r#"params.environment == "staging""#,
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(false)
        );

        // String inequality
        assert_eq!(
            resolve_expressions(
                r#"params.environment != "staging""#,
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions(
                r#"params.environment != "production""#,
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(false)
        );

        // Boolean equality
        assert_eq!(
            resolve_expressions(
                "state.deploy_ready == true",
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions(
                "state.config_loaded == false",
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );
    }

    #[test]
    fn test_resolve_expressions_comparison_operators() {
        let params = create_test_params();
        let state = create_test_state();
        let matrix = create_test_matrix();

        // Numeric comparisons - should return boolean results
        assert_eq!(
            resolve_expressions(
                "params.version > 2.0",
                &params,
                &state,
                Some(&matrix),
                None,
                None
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions(
                "params.version >= 2.1",
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions(
                "state.retry_count < 5",
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions(
                "state.retry_count <= 3",
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions(
                "params.version < 2.0",
                &params,
                &state,
                Some(&matrix),
                None,
                None
            )
            .unwrap(),
            EvalValue::Boolean(false)
        );

        assert_eq!(
            resolve_expressions(
                "state.retry_count > 5",
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(false)
        );
    }

    #[test]
    fn test_resolve_expressions_boolean_operators() {
        let params = create_test_params();
        let state = create_test_state();
        let matrix = create_test_matrix();

        // AND operator
        assert_eq!(
            resolve_expressions(
                r#"params.environment == "production" && params.skip_tests == "false""#,
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions(
                r#"params.environment == "staging" && params.skip_tests == "false""#,
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(false)
        );

        // OR operator
        assert_eq!(
            resolve_expressions(
                r#"params.environment == "production" || params.environment == "staging""#,
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions(
                r#"params.environment == "staging" || params.skip_tests == "false""#,
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions(
                r#"params.environment == "staging" || params.skip_tests == "true""#,
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(false)
        );

        // NOT operator
        assert_eq!(
            resolve_expressions(
                r#"!(params.environment == "staging")"#,
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions(
                "!(state.config_loaded)",
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions(
                "!(state.deploy_ready)",
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(false)
        );
    }

    #[test]
    fn test_resolve_expressions_complex_expressions() {
        let params = create_test_params();
        let state = create_test_state();
        let matrix = create_test_matrix();

        // Complex nested expression
        assert_eq!(
            resolve_expressions(
                r#"(params.environment == "production" || params.environment == "staging") && params.skip_tests != "true" && state.retry_count < 5"#,
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            ).unwrap(),
            EvalValue::Boolean(true)
        );

        // Expression with parentheses
        assert_eq!(
            resolve_expressions(
                r#"!(params.skip_tests == "true") && (state.deploy_ready == true)"#,
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        // Mixed types in expression - use consistent numeric types
        assert_eq!(
            resolve_expressions(
                r#"params.version >= 2.0 && state.deploy_ready == true && matrix.os == "linux""#,
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );
    }

    #[test]
    fn test_resolve_expressions_matrix_values() {
        let params = create_test_params();
        let state = create_test_state();
        let matrix = create_test_matrix();

        // Matrix value access
        assert_eq!(
            resolve_expressions(
                r#"matrix.os == "linux""#,
                &params,
                &state,
                Some(&matrix),
                None,
                None
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        // Numeric matrix value
        assert_eq!(
            resolve_expressions(
                "matrix.node_version == 18",
                &params,
                &state,
                Some(&matrix),
                None,
                None
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        // Boolean matrix value
        assert_eq!(
            resolve_expressions(
                "matrix.parallel == true",
                &params,
                &state,
                Some(&matrix),
                None,
                None
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        // Combined matrix and params
        assert_eq!(
            resolve_expressions(
                r#"matrix.os == "linux" && params.environment == "production""#,
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );
    }

    #[test]
    fn test_resolve_expressions_without_matrix() {
        let params = create_test_params();
        let state = create_test_state();

        // Should work without matrix values
        assert_eq!(
            resolve_expressions(
                r#"params.environment == "production" && state.deploy_ready == true"#,
                &params,
                &state,
                None,
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );
    }

    #[test]
    fn test_resolve_expressions_type_conversions() {
        let params = create_test_params();
        let state = create_test_state();

        // String numbers should work with numeric comparisons
        assert_eq!(
            resolve_expressions("params.max_attempts > 3", &params, &state, None, None, None)
                .unwrap(),
            EvalValue::Boolean(true)
        );

        // Numbers should work with other numbers (not mixed string/number)
        assert_eq!(
            resolve_expressions("state.retry_count == 3", &params, &state, None, None, None)
                .unwrap(),
            EvalValue::Boolean(true)
        );

        // String comparisons should work with strings
        assert_eq!(
            resolve_expressions(
                r#"params.environment == "production""#,
                &params,
                &state,
                None,
                None,
                None,
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );
    }

    #[test]
    fn test_resolve_expressions_empty_and_null_values() {
        let params = create_test_params();
        let state = create_test_state();

        // Empty string should be falsy when compared to boolean
        assert_eq!(
            resolve_expressions(
                "params.empty_param == true",
                &params,
                &state,
                None,
                None,
                None
            )
            .unwrap(),
            EvalValue::Boolean(false)
        );

        // Empty string comparisons
        assert_eq!(
            resolve_expressions(
                r#"params.empty_param == """#,
                &params,
                &state,
                None,
                None,
                None
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        // Note: evalexpr doesn't support null comparisons directly
        // We handle null values by converting them to Empty type
    }

    #[test]
    fn test_resolve_expressions_return_value_types() {
        let params = create_test_params();
        let state = create_test_state();
        let matrix = create_test_matrix();

        // Test that we can return string values
        assert_eq!(
            resolve_expressions(
                "params.environment",
                &params,
                &state,
                Some(&matrix),
                None,
                None
            )
            .unwrap(),
            EvalValue::String("production".to_string())
        );

        // Test that we can return numeric values
        assert_eq!(
            resolve_expressions(
                "state.retry_count",
                &params,
                &state,
                Some(&matrix),
                None,
                None
            )
            .unwrap(),
            EvalValue::Int(3)
        );

        assert_eq!(
            resolve_expressions("params.version", &params, &state, Some(&matrix), None, None)
                .unwrap(),
            EvalValue::Float(2.1)
        );

        // Test that we can return boolean values
        assert_eq!(
            resolve_expressions(
                "state.deploy_ready",
                &params,
                &state,
                Some(&matrix),
                None,
                None
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        // Test arithmetic operations
        assert_eq!(
            resolve_expressions(
                "state.retry_count + 2",
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            EvalValue::Int(5)
        );

        // Test string concatenation (if supported)
        match resolve_expressions(
            r#"params.environment + "-env""#,
            &params,
            &state,
            Some(&matrix),
            None,
            None,
        ) {
            Ok(EvalValue::String(s)) => assert_eq!(s, "production-env"),
            Ok(_) => panic!("Expected string result"),
            Err(_) => {
                // evalexpr might not support string concatenation, which is fine
                // Let's test a simpler expression instead
                assert_eq!(
                    resolve_expressions(
                        "params.max_attempts",
                        &params,
                        &state,
                        Some(&matrix),
                        None,
                        None,
                    )
                    .unwrap(),
                    EvalValue::Float(5.0)
                );
            }
        }
    }

    #[test]
    fn test_evaluate_condition_with_expressions() {
        let params = create_test_params();
        let state = create_test_state();
        let matrix = create_test_matrix();

        // Should use expression evaluation for complex conditions
        assert!(evaluate_condition(
            r#"params.environment == "production" && params.skip_tests != "true""#,
            &params,
            &state,
            Some(&matrix),
            None,
            None,
        )
        .unwrap());

        assert!(!evaluate_condition(
            r#"params.environment == "staging""#,
            &params,
            &state,
            Some(&matrix),
            None,
            None,
        )
        .unwrap());
    }

    #[test]
    fn test_evaluate_condition_handles_non_boolean_values() {
        let params = create_test_params();
        let state = create_test_state();

        // Test that evaluate_condition properly converts non-boolean EvalValues to boolean
        // String values should be truthy if non-empty
        assert!(
            evaluate_condition("params.environment", &params, &state, None, None, None).unwrap()
        );
        assert!(
            !evaluate_condition("params.empty_param", &params, &state, None, None, None).unwrap()
        );

        // Numeric values should be truthy if non-zero
        assert!(
            evaluate_condition("state.retry_count", &params, &state, None, None, None).unwrap()
        );

        // Boolean values should return as-is
        assert!(
            evaluate_condition("state.deploy_ready", &params, &state, None, None, None).unwrap()
        );
        assert!(
            !evaluate_condition("state.config_loaded", &params, &state, None, None, None).unwrap()
        );

        // Test string "false" should be falsy
        let mut test_params = HashMap::new();
        test_params.insert("false_string".to_string(), json!("false"));
        assert!(!evaluate_condition(
            "params.false_string",
            &test_params,
            &HashMap::new(),
            None,
            None,
            None
        )
        .unwrap());
    }

    #[test]
    fn test_evaluate_condition_boolean_conversions() {
        let mut params = HashMap::new();
        params.insert("flag".to_string(), json!("true"));
        params.insert("zero".to_string(), json!("0"));
        params.insert("false_str".to_string(), json!("false"));
        params.insert("empty".to_string(), json!(""));
        params.insert("number".to_string(), json!("42"));

        let state = HashMap::new();

        // Test truthy values using direct variable access
        assert!(evaluate_condition("params.flag", &params, &state, None, None, None).unwrap());
        assert!(evaluate_condition("params.number", &params, &state, None, None, None).unwrap());

        // Test falsy values
        assert!(!evaluate_condition("params.zero", &params, &state, None, None, None).unwrap());
        assert!(
            !evaluate_condition("params.false_str", &params, &state, None, None, None).unwrap()
        );
        assert!(!evaluate_condition("params.empty", &params, &state, None, None, None).unwrap());

        // Test with comparison expressions to ensure they return proper booleans
        assert!(evaluate_condition(
            r#"params.flag == "true""#,
            &params,
            &state,
            None,
            None,
            None
        )
        .unwrap());
        assert!(!evaluate_condition(
            r#"params.flag == "false""#,
            &params,
            &state,
            None,
            None,
            None
        )
        .unwrap());
    }

    #[test]
    fn test_resolve_expressions_error_handling() {
        let params = create_test_params();
        let state = create_test_state();

        // Invalid syntax should return an error
        assert!(resolve_expressions(
            "params.environment == ", // Invalid syntax
            &params,
            &state,
            None,
            None,
            None
        )
        .is_err());

        // Undefined variable should return an error
        assert!(resolve_expressions(
            "params.nonexistent == true",
            &params,
            &state,
            None,
            None,
            None
        )
        .is_err());

        // Test that evaluate_condition handles errors from resolve_expressions properly
        assert!(evaluate_condition(
            "params.nonexistent == true",
            &params,
            &state,
            None,
            None,
            None
        )
        .is_err());
    }

    #[test]
    fn test_resolve_string_with_expression_basic() {
        let mut params = HashMap::new();
        params.insert("name".to_string(), json!("John Doe"));
        params.insert("age".to_string(), json!("25"));

        let state = HashMap::new();

        // Basic variable substitution
        assert_eq!(
            resolve_string_with_expression(
                "Hello ${{ params.name }}",
                &params,
                &state,
                None,
                None,
                None
            )
            .unwrap(),
            "Hello John Doe"
        );

        // Multiple substitutions
        assert_eq!(
            resolve_string_with_expression(
                "Hello ${{ params.name }}, you are ${{ params.age }} years old",
                &params,
                &state,
                None,
                None,
                None,
            )
            .unwrap(),
            "Hello John Doe, you are 25 years old"
        );
    }

    #[test]
    fn test_resolve_string_with_expression_arithmetic() {
        let params = HashMap::new();
        let state = HashMap::new();

        // Basic arithmetic
        assert_eq!(
            resolve_string_with_expression(
                "Result: ${{ 1 + 2 }}",
                &params,
                &state,
                None,
                None,
                None
            )
            .unwrap(),
            "Result: 3"
        );

        // More complex arithmetic
        assert_eq!(
            resolve_string_with_expression(
                "Calculation: ${{ 10 * 3 - 5 }}",
                &params,
                &state,
                None,
                None,
                None,
            )
            .unwrap(),
            "Calculation: 25"
        );

        // Float arithmetic
        assert_eq!(
            resolve_string_with_expression(
                "Float: ${{ 2.5 + 1.5 }}",
                &params,
                &state,
                None,
                None,
                None
            )
            .unwrap(),
            "Float: 4"
        );
    }

    #[test]
    fn test_resolve_string_with_expression_with_params_and_state() {
        let mut params = HashMap::new();
        params.insert("base".to_string(), json!("10"));
        params.insert("multiplier".to_string(), json!("3"));

        let mut state = HashMap::new();
        state.insert("bonus".to_string(), json!(5));

        // Mixed arithmetic with params and state
        assert_eq!(
            resolve_string_with_expression(
                "Total: ${{ params.base * params.multiplier + state.bonus }}",
                &params,
                &state,
                None,
                None,
                None,
            )
            .unwrap(),
            "Total: 35"
        );
    }

    #[test]
    fn test_resolve_string_with_expression_boolean_and_comparison() {
        let mut params = HashMap::new();
        params.insert("environment".to_string(), json!("production"));

        let mut state = HashMap::new();
        state.insert("ready".to_string(), json!(true));
        state.insert("count".to_string(), json!(5));

        // Boolean values
        assert_eq!(
            resolve_string_with_expression(
                "Ready: ${{ state.ready }}",
                &params,
                &state,
                None,
                None,
                None,
            )
            .unwrap(),
            "Ready: true"
        );

        // Comparison expressions
        assert_eq!(
            resolve_string_with_expression(
                "Is production: ${{ params.environment == \"production\" }}",
                &params,
                &state,
                None,
                None,
                None,
            )
            .unwrap(),
            "Is production: true"
        );

        // Complex boolean expression
        assert_eq!(
            resolve_string_with_expression(
                "Deploy ready: ${{ params.environment == \"production\" && state.ready == true && state.count >= 5 }}",
                &params,
                &state,
                None,
                None,
                None,
            ).unwrap(),
            "Deploy ready: true"
        );
    }

    #[test]
    fn test_resolve_string_with_expression_with_matrix_values() {
        let params = HashMap::new();
        let state = HashMap::new();
        let mut matrix = HashMap::new();
        matrix.insert("os".to_string(), json!("linux"));
        matrix.insert("version".to_string(), json!(18));

        // Matrix value substitution
        assert_eq!(
            resolve_string_with_expression(
                "Running on ${{ matrix.os }} with version ${{ matrix.version }}",
                &params,
                &state,
                Some(&matrix),
                None,
                None,
            )
            .unwrap(),
            "Running on linux with version 18"
        );
    }

    #[test]
    fn test_resolve_string_with_expression_whitespace() {
        let mut params = HashMap::new();
        params.insert("name".to_string(), json!("Alice"));

        let state = HashMap::new();

        // Test whitespace handling inside expressions
        assert_eq!(
            resolve_string_with_expression(
                "Hello ${{    params.name    }}",
                &params,
                &state,
                None,
                None,
                None,
            )
            .unwrap(),
            "Hello Alice"
        );

        assert_eq!(
            resolve_string_with_expression(
                "Math: ${{  1  +  2  }}",
                &params,
                &state,
                None,
                None,
                None
            )
            .unwrap(),
            "Math: 3"
        );
    }

    #[test]
    fn test_resolve_string_with_expression_no_expressions() {
        let params = HashMap::new();
        let state = HashMap::new();

        // String with no expressions should be returned as-is
        assert_eq!(
            resolve_string_with_expression("Hello world", &params, &state, None, None, None)
                .unwrap(),
            "Hello world"
        );

        // String with similar but not exact syntax
        assert_eq!(
            resolve_string_with_expression(
                "Not an expression: {{ params.name }}",
                &params,
                &state,
                None,
                None,
                None,
            )
            .unwrap(),
            "Not an expression: {{ params.name }}"
        );
    }

    #[test]
    fn test_resolve_string_with_expression_error_handling() {
        let params = HashMap::new();
        let state = HashMap::new();

        // Invalid expression should return error
        assert!(resolve_string_with_expression(
            "Invalid: ${{ params.nonexistent }}",
            &params,
            &state,
            None,
            None,
            None
        )
        .is_err());

        // Invalid syntax should return error
        assert!(resolve_string_with_expression(
            "Invalid syntax: ${{ 1 + }}",
            &params,
            &state,
            None,
            None,
            None
        )
        .is_err());
    }

    #[test]
    fn test_resolve_string_with_expression_empty_values() {
        let mut params = HashMap::new();
        params.insert("empty".to_string(), json!(""));

        let mut state = HashMap::new();
        state.insert("null_value".to_string(), json!(null));

        // Empty string
        assert_eq!(
            resolve_string_with_expression(
                "Value: '${{ params.empty }}'",
                &params,
                &state,
                None,
                None,
                None,
            )
            .unwrap(),
            "Value: ''"
        );

        // Null value should become empty string
        assert_eq!(
            resolve_string_with_expression(
                "Null: '${{ state.null_value }}'",
                &params,
                &state,
                None,
                None,
                None,
            )
            .unwrap(),
            "Null: ''"
        );
    }

    #[test]
    fn test_resolve_string_with_expression_with_missing_params() {
        let params = HashMap::new();
        let state = HashMap::new();

        assert!(resolve_string_with_expression(
            "Hello ${{ params.name }}",
            &params,
            &state,
            None,
            None,
            None
        )
        .is_err());
    }

    #[test]
    fn test_resolve_string_list_literal_values() {
        let params = HashMap::new();
        let state = HashMap::new();
        let items = vec!["**/*.js".to_string(), "**/*.ts".to_string()];

        let result = resolve_string_list(&items, &params, &state, None, None, None).unwrap();
        assert_eq!(result, vec!["**/*.js", "**/*.ts"]);
    }

    #[test]
    fn test_resolve_string_list_expands_state_array() {
        let params = HashMap::new();
        let mut state = HashMap::new();
        state.insert(
            "files".to_string(),
            json!(["src/a.js", "src/b.js", "src/c.js"]),
        );

        let items = vec!["${{ state.files }}".to_string()];
        let result = resolve_string_list(&items, &params, &state, None, None, None).unwrap();
        assert_eq!(result, vec!["src/a.js", "src/b.js", "src/c.js"]);
    }

    #[test]
    fn test_resolve_string_list_mixed_literal_and_array() {
        let params = HashMap::new();
        let mut state = HashMap::new();
        state.insert("extra".to_string(), json!(["lib/**/*.ts"]));

        let items = vec!["**/*.js".to_string(), "${{ state.extra }}".to_string()];
        let result = resolve_string_list(&items, &params, &state, None, None, None).unwrap();
        assert_eq!(result, vec!["**/*.js", "lib/**/*.ts"]);
    }

    #[test]
    fn test_resolve_string_list_scalar_expression() {
        let mut params = HashMap::new();
        params.insert("glob".to_string(), json!("src/**/*.tsx"));
        let state = HashMap::new();

        let items = vec!["${{ params.glob }}".to_string()];
        let result = resolve_string_list(&items, &params, &state, None, None, None).unwrap();
        assert_eq!(result, vec!["src/**/*.tsx"]);
    }

    #[test]
    fn test_resolve_string_list_json_encoded_array_string() {
        let params = HashMap::new();
        let mut state = HashMap::new();
        // Simulates a CODEMOD_STATE_* env var that was JSON-encoded
        state.insert("batch".to_string(), json!(r#"["file1.js","file2.js"]"#));

        let items = vec!["${{ state.batch }}".to_string()];
        let result = resolve_string_list(&items, &params, &state, None, None, None).unwrap();
        assert_eq!(result, vec!["file1.js", "file2.js"]);
    }

    #[test]
    fn test_resolve_string_list_matrix_array() {
        let params = HashMap::new();
        let state = HashMap::new();
        let mut matrix = HashMap::new();
        matrix.insert("files".to_string(), json!(["a.ts", "b.ts"]));

        let items = vec!["${{ matrix.files }}".to_string()];
        let result =
            resolve_string_list(&items, &params, &state, Some(&matrix), None, None).unwrap();
        assert_eq!(result, vec!["a.ts", "b.ts"]);
    }

    #[test]
    fn test_resolve_string_list_empty_on_missing_var() {
        let params = HashMap::new();
        let state = HashMap::new();

        // Missing task var should resolve to empty and be filtered out
        let items = vec!["${{ task.missing }}".to_string()];
        let result = resolve_string_list(&items, &params, &state, None, None, None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_value_to_string_vec_array() {
        let val = json!(["a", "b", "c"]);
        assert_eq!(
            value_to_string_vec(&val),
            Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
        );
    }

    #[test]
    fn test_value_to_string_vec_json_string() {
        let val = json!(r#"["x","y"]"#);
        assert_eq!(
            value_to_string_vec(&val),
            Some(vec!["x".to_string(), "y".to_string()])
        );
    }

    #[test]
    fn test_value_to_string_vec_non_array() {
        assert_eq!(value_to_string_vec(&json!("hello")), None);
        assert_eq!(value_to_string_vec(&json!(42)), None);
        assert_eq!(value_to_string_vec(&json!(true)), None);
    }
}
