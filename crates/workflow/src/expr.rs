//! `evaluate(kind, expression, scope)`: evaluates `${{ ... }}` expressions
//! from workflow.yaml with the same evaluator the YAML engine used
//! (`butterflow_models::variable`), so translated workflows keep their exact
//! semantics.

use std::collections::HashMap;

use butterflow_models::variable::{
    evaluate_condition, resolve_string_list, resolve_string_with_expression,
};
use serde::Deserialize;
use serde_json::Value;

use crate::run::RunContext;

#[derive(Deserialize, Default)]
struct Scope {
    #[serde(default)]
    params: HashMap<String, Value>,
    #[serde(default)]
    state: HashMap<String, Value>,
    #[serde(default)]
    matrix: Option<HashMap<String, Value>>,
}

/// Returns JSON: a boolean for `condition`, a string for `string`, and an
/// array of strings for `list` (whose expression is a JSON array of items).
pub(crate) fn evaluate(
    _run: &RunContext,
    kind: &str,
    expression: &str,
    scope: &str,
) -> Result<String, String> {
    let scope: Scope = serde_json::from_str(scope).map_err(|e| js_error(e.to_string()))?;
    let matrix = scope.matrix.as_ref();
    let value = match kind {
        "condition" => {
            evaluate_condition(expression, &scope.params, &scope.state, matrix, None, None)
                .map(Value::Bool)
        }
        "string" => resolve_string_with_expression(
            expression,
            &scope.params,
            &scope.state,
            matrix,
            None,
            None,
        )
        .map(Value::String),
        "list" => {
            let items: Vec<String> =
                serde_json::from_str(expression).map_err(|e| js_error(e.to_string()))?;
            resolve_string_list(&items, &scope.params, &scope.state, matrix, None, None)
                .map(Value::from)
        }
        other => return Err(js_error(format!("unknown expression kind '{other}'"))),
    }
    .map_err(|e| js_error(format!("cannot evaluate '{expression}': {e}")))?;
    Ok(value.to_string())
}

fn js_error(message: String) -> String {
    message
}
