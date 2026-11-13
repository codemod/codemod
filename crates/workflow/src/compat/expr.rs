//! Translating workflow.yaml `${{ ... }}` expressions to TypeScript.
//!
//! Simple expressions become plain TypeScript. Anything else falls back to
//! `evaluate(...)`, which runs the YAML engine's own evaluator at run time,
//! so the semantics never change.

use std::sync::OnceLock;

use regex::Regex;

fn expr_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\$\{\{\s*([^}]+?)\s*\}\}").expect("valid regex"))
}

/// The scope object passed to `evaluate`.
pub const SCOPE: &str = "{ params, matrix, state: getState() }";

/// A JavaScript string literal.
pub fn js_string(text: &str) -> String {
    serde_json::to_string(text).expect("strings serialize")
}

/// A template string with `${{ }}` expressions, as a TypeScript expression
/// that evaluates to a string.
pub fn template(text: &str) -> String {
    if !text.contains("${{") {
        return js_string(text);
    }
    let mut out = String::from("`");
    let mut last = 0;
    for captures in expr_re().captures_iter(text) {
        let whole = captures.get(0).unwrap();
        out.push_str(&escape_template(&text[last..whole.start()]));
        let inner = captures.get(1).unwrap().as_str();
        match path(inner) {
            Some(js) => out.push_str(&format!("${{{js}}}")),
            None => out.push_str(&format!(
                "${{evaluate(\"string\", {}, {SCOPE})}}",
                js_string(whole.as_str())
            )),
        }
        last = whole.end();
    }
    out.push_str(&escape_template(&text[last..]));
    out.push('`');
    out
}

/// A list of glob strings that may contain `${{ }}` (an array-valued
/// expression expands into several entries, as in the YAML engine).
pub fn string_list(items: &[String]) -> String {
    if items.iter().any(|item| item.contains("${{")) {
        format!(
            "evaluate(\"list\", {}, {SCOPE})",
            js_string(&serde_json::to_string(items).expect("strings serialize"))
        )
    } else {
        let items: Vec<String> = items.iter().map(|item| js_string(item)).collect();
        format!("[{}]", items.join(", "))
    }
}

/// An `if:` condition as a TypeScript boolean expression. It always runs
/// through the YAML engine's evaluator: TypeScript differs from it on
/// unbound variables, number and string comparisons and truthiness, and a
/// condition that cannot be evaluated skips its step, as it always has.
pub fn condition(text: &str) -> String {
    let trimmed = text.trim();
    format!("evaluate(\"condition\", {}, {SCOPE})", js_string(trimmed))
}

fn escape_template(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${")
}

/// `params.a.b`, `matrix.x` or `state.x.y` as TypeScript; `None` for
/// anything else.
fn path(expression: &str) -> Option<String> {
    let expression = expression.trim();
    let mut parts = expression.split('.');
    let root = parts.next()?;
    let rest: Vec<&str> = parts.collect();
    if rest.is_empty() || !rest.iter().all(|part| is_identifier(part)) {
        return None;
    }
    let base = match root {
        "params" => format!("params.{}", rest[0]),
        "matrix" => format!("matrix?.{}", rest[0]),
        // State has no declared types in workflow.yaml, so it is read as `any`.
        "state" => format!("getState<any>({})", js_string(rest[0])),
        _ => return None,
    };
    Some(
        rest[1..]
            .iter()
            .fold(base, |acc, part| format!("{acc}?.{part}")),
    )
}

fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_paths_become_template_literals() {
        assert_eq!(template("plain"), "\"plain\"");
        assert_eq!(
            template("migrate ${{ matrix.name }} for ${{ params.team.id }}"),
            "`migrate ${matrix?.name} for ${params.team?.id}`"
        );
        assert_eq!(
            template("${{ state.count }}`"),
            "`${getState<any>(\"count\")}\\``"
        );
    }

    #[test]
    fn other_expressions_fall_back_to_the_evaluator() {
        assert_eq!(
            template("n=${{ 1 + params.n }}"),
            "`n=${evaluate(\"string\", \"${{ 1 + params.n }}\", { params, matrix, state: getState() })}`"
        );
    }

    #[test]
    fn conditions_run_through_the_evaluator() {
        assert_eq!(
            condition("${{ params.mode == \"fast\" }}"),
            "evaluate(\"condition\", \"${{ params.mode == \\\"fast\\\" }}\", { params, matrix, state: getState() })"
        );
        assert!(condition("state.count > 0").starts_with("evaluate(\"condition\""));
    }

    #[test]
    fn lists_with_expressions_use_the_evaluator() {
        assert_eq!(string_list(&["**/*.ts".into()]), "[\"**/*.ts\"]");
        assert!(string_list(&["${{ state.files }}".into()]).starts_with("evaluate(\"list\""));
    }
}
