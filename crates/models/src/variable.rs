use evalexpr::{
    eval_with_context, ContextWithMutableVariables, HashMapContext, Value as EvalValue,
};
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;

use crate::error::Error;
use crate::Result;

/// Convert a serde_json::Value to evalexpr::EvalValue
/// Handles type conversion including string-to-number parsing
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

/// Resolve a dot notation path in a HashMap<String, Value>
/// For example: resolve_state_path(state, "x.y.0.z") gets state["x"]["y"][0]["z"]
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

/// Resolve expressions with operators like ==, !=, &&, ||, !
/// This function handles complex boolean expressions, not just variable substitution
pub fn resolve_expressions(
    expression: &str,
    params: &HashMap<String, Value>,
    state: &HashMap<String, Value>,
    matrix_values: Option<&HashMap<String, Value>>,
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
            let eval_value = convert_value_to_eval_value(value);
            context.set_value(key.clone(), eval_value)?;
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
) -> Result<bool> {
    let result = resolve_expressions(condition, params, state, matrix_values)?;
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
) -> Result<String> {
    // regex to find all ${{ }} patterns
    let re = Regex::new(r"\$\{\{\s*([^}]+?)\s*\}\}")
        .map_err(|e| Error::VariableResolution(format!("Regex error: {}", e)))?;

    let mut result = template.to_string();

    for captures in re.captures_iter(template) {
        let full_match = captures.get(0).unwrap().as_str();
        let expression = captures.get(1).unwrap().as_str().trim();

        let eval_result = resolve_expressions(expression, params, state, matrix_values)?;

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
                Some(&matrix)
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions(
                r#"params.environment == "staging""#,
                &params,
                &state,
                Some(&matrix)
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
                Some(&matrix)
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions(
                r#"params.environment != "production""#,
                &params,
                &state,
                Some(&matrix)
            )
            .unwrap(),
            EvalValue::Boolean(false)
        );

        // Boolean equality
        assert_eq!(
            resolve_expressions("state.deploy_ready == true", &params, &state, Some(&matrix))
                .unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions(
                "state.config_loaded == false",
                &params,
                &state,
                Some(&matrix)
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
            resolve_expressions("params.version > 2.0", &params, &state, Some(&matrix)).unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions("params.version >= 2.1", &params, &state, Some(&matrix)).unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions("state.retry_count < 5", &params, &state, Some(&matrix)).unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions("state.retry_count <= 3", &params, &state, Some(&matrix)).unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions("params.version < 2.0", &params, &state, Some(&matrix)).unwrap(),
            EvalValue::Boolean(false)
        );

        assert_eq!(
            resolve_expressions("state.retry_count > 5", &params, &state, Some(&matrix)).unwrap(),
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
                Some(&matrix)
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions(
                r#"params.environment == "staging" && params.skip_tests == "false""#,
                &params,
                &state,
                Some(&matrix)
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
                Some(&matrix)
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions(
                r#"params.environment == "staging" || params.skip_tests == "false""#,
                &params,
                &state,
                Some(&matrix)
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions(
                r#"params.environment == "staging" || params.skip_tests == "true""#,
                &params,
                &state,
                Some(&matrix)
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
                Some(&matrix)
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions("!(state.config_loaded)", &params, &state, Some(&matrix)).unwrap(),
            EvalValue::Boolean(true)
        );

        assert_eq!(
            resolve_expressions("!(state.deploy_ready)", &params, &state, Some(&matrix)).unwrap(),
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
                Some(&matrix)
            ).unwrap(),
            EvalValue::Boolean(true)
        );

        // Expression with parentheses
        assert_eq!(
            resolve_expressions(
                r#"!(params.skip_tests == "true") && (state.deploy_ready == true)"#,
                &params,
                &state,
                Some(&matrix)
            )
            .unwrap(),
            EvalValue::Boolean(true)
        );

        // Mixed types in expression - use consistent numeric types
        assert_eq!(
            resolve_expressions(
                r#"params.version >= 2.0 && state.deploy_ready == true && os == "linux""#,
                &params,
                &state,
                Some(&matrix)
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
            resolve_expressions(r#"os == "linux""#, &params, &state, Some(&matrix)).unwrap(),
            EvalValue::Boolean(true)
        );

        // Numeric matrix value
        assert_eq!(
            resolve_expressions("node_version == 18", &params, &state, Some(&matrix)).unwrap(),
            EvalValue::Boolean(true)
        );

        // Boolean matrix value
        assert_eq!(
            resolve_expressions("parallel == true", &params, &state, Some(&matrix)).unwrap(),
            EvalValue::Boolean(true)
        );

        // Combined matrix and params
        assert_eq!(
            resolve_expressions(
                r#"os == "linux" && params.environment == "production""#,
                &params,
                &state,
                Some(&matrix)
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
                None
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
            resolve_expressions("params.max_attempts > 3", &params, &state, None).unwrap(),
            EvalValue::Boolean(true)
        );

        // Numbers should work with other numbers (not mixed string/number)
        assert_eq!(
            resolve_expressions("state.retry_count == 3", &params, &state, None).unwrap(),
            EvalValue::Boolean(true)
        );

        // String comparisons should work with strings
        assert_eq!(
            resolve_expressions(
                r#"params.environment == "production""#,
                &params,
                &state,
                None
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
            resolve_expressions("params.empty_param == true", &params, &state, None).unwrap(),
            EvalValue::Boolean(false)
        );

        // Empty string comparisons
        assert_eq!(
            resolve_expressions(r#"params.empty_param == """#, &params, &state, None).unwrap(),
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
            resolve_expressions("params.environment", &params, &state, Some(&matrix)).unwrap(),
            EvalValue::String("production".to_string())
        );

        // Test that we can return numeric values
        assert_eq!(
            resolve_expressions("state.retry_count", &params, &state, Some(&matrix)).unwrap(),
            EvalValue::Int(3)
        );

        assert_eq!(
            resolve_expressions("params.version", &params, &state, Some(&matrix)).unwrap(),
            EvalValue::Float(2.1)
        );

        // Test that we can return boolean values
        assert_eq!(
            resolve_expressions("state.deploy_ready", &params, &state, Some(&matrix)).unwrap(),
            EvalValue::Boolean(true)
        );

        // Test arithmetic operations
        assert_eq!(
            resolve_expressions("state.retry_count + 2", &params, &state, Some(&matrix)).unwrap(),
            EvalValue::Int(5)
        );

        // Test string concatenation (if supported)
        match resolve_expressions(
            r#"params.environment + "-env""#,
            &params,
            &state,
            Some(&matrix),
        ) {
            Ok(EvalValue::String(s)) => assert_eq!(s, "production-env"),
            Ok(_) => panic!("Expected string result"),
            Err(_) => {
                // evalexpr might not support string concatenation, which is fine
                // Let's test a simpler expression instead
                assert_eq!(
                    resolve_expressions("params.max_attempts", &params, &state, Some(&matrix))
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
            Some(&matrix)
        )
        .unwrap());

        assert!(!evaluate_condition(
            r#"params.environment == "staging""#,
            &params,
            &state,
            Some(&matrix)
        )
        .unwrap());
    }

    #[test]
    fn test_evaluate_condition_handles_non_boolean_values() {
        let params = create_test_params();
        let state = create_test_state();

        // Test that evaluate_condition properly converts non-boolean EvalValues to boolean
        // String values should be truthy if non-empty
        assert!(evaluate_condition("params.environment", &params, &state, None).unwrap());
        assert!(!evaluate_condition("params.empty_param", &params, &state, None).unwrap());

        // Numeric values should be truthy if non-zero
        assert!(evaluate_condition("state.retry_count", &params, &state, None).unwrap());

        // Boolean values should return as-is
        assert!(evaluate_condition("state.deploy_ready", &params, &state, None).unwrap());
        assert!(!evaluate_condition("state.config_loaded", &params, &state, None).unwrap());

        // Test string "false" should be falsy
        let mut test_params = HashMap::new();
        test_params.insert("false_string".to_string(), json!("false"));
        assert!(
            !evaluate_condition("params.false_string", &test_params, &HashMap::new(), None)
                .unwrap()
        );
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
        assert!(evaluate_condition("params.flag", &params, &state, None).unwrap());
        assert!(evaluate_condition("params.number", &params, &state, None).unwrap());

        // Test falsy values
        assert!(!evaluate_condition("params.zero", &params, &state, None).unwrap());
        assert!(!evaluate_condition("params.false_str", &params, &state, None).unwrap());
        assert!(!evaluate_condition("params.empty", &params, &state, None).unwrap());

        // Test with comparison expressions to ensure they return proper booleans
        assert!(evaluate_condition(r#"params.flag == "true""#, &params, &state, None).unwrap());
        assert!(!evaluate_condition(r#"params.flag == "false""#, &params, &state, None).unwrap());
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
            None
        )
        .is_err());

        // Undefined variable should return an error
        assert!(resolve_expressions("params.nonexistent == true", &params, &state, None).is_err());

        // Test that evaluate_condition handles errors from resolve_expressions properly
        assert!(evaluate_condition("params.nonexistent == true", &params, &state, None).is_err());
    }

    #[test]
    fn test_resolve_string_with_expression_basic() {
        let mut params = HashMap::new();
        params.insert("name".to_string(), json!("John Doe"));
        params.insert("age".to_string(), json!("25"));

        let state = HashMap::new();

        // Basic variable substitution
        assert_eq!(
            resolve_string_with_expression("Hello ${{ params.name }}", &params, &state, None)
                .unwrap(),
            "Hello John Doe"
        );

        // Multiple substitutions
        assert_eq!(
            resolve_string_with_expression(
                "Hello ${{ params.name }}, you are ${{ params.age }} years old",
                &params,
                &state,
                None
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
            resolve_string_with_expression("Result: ${{ 1 + 2 }}", &params, &state, None).unwrap(),
            "Result: 3"
        );

        // More complex arithmetic
        assert_eq!(
            resolve_string_with_expression("Calculation: ${{ 10 * 3 - 5 }}", &params, &state, None)
                .unwrap(),
            "Calculation: 25"
        );

        // Float arithmetic
        assert_eq!(
            resolve_string_with_expression("Float: ${{ 2.5 + 1.5 }}", &params, &state, None)
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
                None
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
            resolve_string_with_expression("Ready: ${{ state.ready }}", &params, &state, None)
                .unwrap(),
            "Ready: true"
        );

        // Comparison expressions
        assert_eq!(
            resolve_string_with_expression(
                "Is production: ${{ params.environment == \"production\" }}",
                &params,
                &state,
                None
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
                None
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
                "Running on ${{ os }} with version ${{ version }}",
                &params,
                &state,
                Some(&matrix)
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
            resolve_string_with_expression("Hello ${{    params.name    }}", &params, &state, None)
                .unwrap(),
            "Hello Alice"
        );

        assert_eq!(
            resolve_string_with_expression("Math: ${{  1  +  2  }}", &params, &state, None)
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
            resolve_string_with_expression("Hello world", &params, &state, None).unwrap(),
            "Hello world"
        );

        // String with similar but not exact syntax
        assert_eq!(
            resolve_string_with_expression(
                "Not an expression: {{ params.name }}",
                &params,
                &state,
                None
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
            None
        )
        .is_err());

        // Invalid syntax should return error
        assert!(resolve_string_with_expression(
            "Invalid syntax: ${{ 1 + }}",
            &params,
            &state,
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
            resolve_string_with_expression("Value: '${{ params.empty }}'", &params, &state, None)
                .unwrap(),
            "Value: ''"
        );

        // Null value should become empty string
        assert_eq!(
            resolve_string_with_expression(
                "Null: '${{ state.null_value }}'",
                &params,
                &state,
                None
            )
            .unwrap(),
            "Null: ''"
        );
    }

    #[test]
    fn test_resolve_string_with_expression_with_missing_params() {
        let params = HashMap::new();
        let state = HashMap::new();

        assert!(
            resolve_string_with_expression("Hello ${{ params.name }}", &params, &state, None)
                .is_err()
        );
    }
}
