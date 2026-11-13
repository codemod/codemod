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

use crate::events::Event;
use crate::run::RunContext;

/// `params` and `state` may be null: a run started without parameters.
#[derive(Deserialize, Default)]
struct Scope {
    #[serde(default, deserialize_with = "null_as_empty")]
    params: HashMap<String, Value>,
    #[serde(default, deserialize_with = "null_as_empty")]
    state: HashMap<String, Value>,
    #[serde(default)]
    matrix: Option<HashMap<String, Value>>,
}

/// Returns JSON: a boolean for `condition`, a string for `string`, and an
/// array of strings for `list` (whose expression is a JSON array of items).
///
/// A condition that cannot be evaluated (an unbound parameter, say) is false,
/// so its step is skipped, as in the YAML engine; the run's log says why.
pub(crate) fn evaluate(
    run: &RunContext,
    kind: &str,
    expression: &str,
    scope: &str,
) -> Result<String, String> {
    let scope: Scope = serde_json::from_str(scope).map_err(|e| e.to_string())?;
    let matrix = scope.matrix.as_ref();
    let value = match kind {
        "condition" => {
            let value =
                evaluate_condition(expression, &scope.params, &scope.state, matrix, None, None)
                    .unwrap_or_else(|error| {
                        run.events.emit(Event::RunLog {
                            line: format!(
                                "warning: skipping a step: its condition '{expression}' cannot be evaluated: {error}"
                            ),
                        });
                        false
                    });
            Ok(Value::Bool(value))
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
                serde_json::from_str(expression).map_err(|e| e.to_string())?;
            resolve_string_list(&items, &scope.params, &scope.state, matrix, None, None)
                .map(Value::from)
        }
        other => return Err(format!("unknown expression kind '{other}'")),
    }
    .map_err(|e| format!("cannot evaluate '{expression}': {e}"))?;
    Ok(value.to_string())
}

fn null_as_empty<'de, D>(deserializer: D) -> Result<HashMap<String, Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::deserialize(deserializer)?.unwrap_or_default())
}
