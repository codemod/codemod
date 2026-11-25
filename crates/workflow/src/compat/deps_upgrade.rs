//! `dependencyGraph()` to `dependencies()`, for `codemod workflow upgrade`:
//! a mechanical rewrite that keeps what the workflow does.
//!
//! - `dependencyGraph(callback)` becomes `() => (callback)(dependencies())`,
//!   a function step that calls the callback with the same object;
//! - `dependencyGraph(options, callback)` passes `options` on, as
//!   `dependencies(options)` (`{}` is dropped);
//! - the import names `dependencies` instead, and a `DependencyGraph` type
//!   import becomes `Dependencies as DependencyGraph`.
//!
//! Step results stay the same: nodes and edges are still recorded as text.

use oxc::allocator::Allocator;
use oxc::ast::ast::{Argument, Expression, ImportDeclarationSpecifier};
use oxc::ast::AstKind;
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::{GetSpan, SourceType};

/// `source` with its `dependencyGraph` calls rewritten; `None` when it has
/// none (or cannot be parsed).
pub fn upgrade_dependency_graph(source: &str) -> Option<String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
    if !parsed.errors.is_empty() {
        return None;
    }
    let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
    let scoping = semantic.scoping();
    let mut edits: Vec<(u32, u32, String)> = Vec::new();
    let mut graph_symbols = Vec::new();
    let mut imports_dependencies = false;

    for node in semantic.nodes().iter() {
        let AstKind::ImportDeclaration(import) = node.kind() else {
            continue;
        };
        if !matches!(
            import.source.value.as_str(),
            "codemod/deps" | "codemod:deps"
        ) {
            continue;
        }
        for specifier in import.specifiers.iter().flatten() {
            if let ImportDeclarationSpecifier::ImportSpecifier(s) = specifier {
                if s.imported.name() == "dependencies" {
                    imports_dependencies = true;
                }
            }
        }
    }

    for node in semantic.nodes().iter() {
        match node.kind() {
            AstKind::ImportDeclaration(import)
                if matches!(
                    import.source.value.as_str(),
                    "codemod/deps" | "codemod:deps"
                ) =>
            {
                for specifier in import.specifiers.iter().flatten() {
                    let ImportDeclarationSpecifier::ImportSpecifier(s) = specifier else {
                        continue;
                    };
                    let local = s.local.name.as_str();
                    match s.imported.name().as_str() {
                        "dependencyGraph" => {
                            graph_symbols.push(s.local.symbol_id());
                            let text = if imports_dependencies {
                                String::new()
                            } else if local == "dependencyGraph" {
                                "dependencies".to_string()
                            } else {
                                format!("dependencies as {local}")
                            };
                            let (start, end) = if text.is_empty() {
                                // Drop the specifier and the comma after it.
                                let after = &source[s.span.end as usize..];
                                let comma = after
                                    .find(|c: char| !c.is_whitespace())
                                    .filter(|&at| after[at..].starts_with(','))
                                    .map(|at| {
                                        at + 1 + after[at + 1..].len()
                                            - after[at + 1..].trim_start().len()
                                    });
                                (s.span.start, s.span.end + comma.unwrap_or(0) as u32)
                            } else {
                                (s.span.start, s.span.end)
                            };
                            edits.push((start, end, text));
                        }
                        "DependencyGraph" => {
                            let kind = if s.import_kind.is_type() { "type " } else { "" };
                            edits.push((
                                s.span.start,
                                s.span.end,
                                format!("{kind}Dependencies as {local}"),
                            ));
                        }
                        _ => {}
                    }
                }
            }
            AstKind::CallExpression(call) => {
                let Expression::Identifier(callee) = &call.callee else {
                    continue;
                };
                let Some(symbol) = scoping.get_reference(callee.reference_id()).symbol_id() else {
                    continue;
                };
                if !graph_symbols.contains(&symbol) {
                    continue;
                }
                let text = |argument: &Argument| {
                    let span = argument.span();
                    source[span.start as usize..span.end as usize].to_string()
                };
                let (options, callback) = match call.arguments.as_slice() {
                    [callback] => (None, text(callback)),
                    [options, callback] => (Some(text(options)), text(callback)),
                    _ => continue,
                };
                let options = options.filter(|o| o.split_whitespace().collect::<String>() != "{}");
                let name = if imports_dependencies {
                    "dependencies".to_string()
                } else {
                    callee.name.to_string()
                };
                let name = if name == "dependencyGraph" {
                    "dependencies".to_string()
                } else {
                    name
                };
                edits.push((
                    call.span.start,
                    call.span.end,
                    format!(
                        "() => ({callback})({name}({}))",
                        options.unwrap_or_default()
                    ),
                ));
            }
            _ => {}
        }
    }
    if edits.is_empty() {
        return None;
    }
    // Imports come before their calls in the walk; apply from the end.
    edits.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    let mut out = source.to_string();
    for (start, end, text) in edits {
        out.replace_range(start as usize..end as usize, &text);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::upgrade_dependency_graph as upgrade;

    #[test]
    fn a_graph_step_becomes_a_function_step() {
        let before = r#"import { step } from "codemod/workflow";
import { dependencyGraph, semver } from "codemod/deps";
import type { DependencyGraph, PackageNode } from "codemod/deps";
export default async function main() {
  const a = await step(dependencyGraph((g) => g.chains({ to: g.nodes("moment") })));
  const b = await step("B", dependencyGraph({}, (g: DependencyGraph) => g.workspaces()));
  const c = await step(dependencyGraph({ lockfiles: "**/yarn.lock" }, async (g) => g.lockfiles));
  return [a, b, c, semver.compare("1.0.0", "2.0.0")];
}
"#;
        assert_eq!(
            upgrade(before).unwrap(),
            r#"import { step } from "codemod/workflow";
import { dependencies, semver } from "codemod/deps";
import type { Dependencies as DependencyGraph, PackageNode } from "codemod/deps";
export default async function main() {
  const a = await step(() => ((g) => g.chains({ to: g.nodes("moment") }))(dependencies()));
  const b = await step("B", () => ((g: DependencyGraph) => g.workspaces())(dependencies()));
  const c = await step(() => (async (g) => g.lockfiles)(dependencies({ lockfiles: "**/yarn.lock" })));
  return [a, b, c, semver.compare("1.0.0", "2.0.0")];
}
"#
        );
    }

    #[test]
    fn a_file_without_dependency_graph_is_left_alone() {
        assert_eq!(upgrade("import { semver } from \"codemod/deps\";\n"), None);
        assert_eq!(
            upgrade("const dependencyGraph = (f) => f; dependencyGraph(() => 1);"),
            None
        );
    }
}
