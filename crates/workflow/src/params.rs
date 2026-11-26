//! The parameters a workflow declares: the members of the type its context
//! takes (`WorkflowContext<Params>`), with their doc comments, so a launcher
//! can offer them for editing.

use oxc::allocator::Allocator;
use oxc::ast::ast::{Declaration, PropertyKey, Statement, TSSignature, TSType};
use oxc::parser::Parser;
use oxc::span::{GetSpan, SourceType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredParam {
    pub name: String,
    /// The member's type as written (`string`, `boolean | string`).
    pub type_text: String,
    pub optional: bool,
    /// Its doc comment, without the comment markers and tags.
    pub description: Option<String>,
    /// Its doc comment's `@default` tag: JSON when it parses as JSON
    /// (`@default 2`, `@default "api"`), else the text.
    pub default: Option<serde_json::Value>,
}

/// The parameters `source` declares, in declaration order; none when it
/// declares no type for them or does not parse.
pub fn declared_params(source: &str) -> Vec<DeclaredParam> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
    if !parsed.errors.is_empty() {
        return Vec::new();
    }
    let program = parsed.program;
    let name = context_type_name(source).unwrap_or_else(|| "Params".to_string());

    let declarations = program.body.iter().filter_map(|statement| match statement {
        Statement::ExportNamedDeclaration(export) => export.declaration.as_ref(),
        other => other.as_declaration(),
    });
    for declaration in declarations {
        let members = match declaration {
            Declaration::TSInterfaceDeclaration(interface) if interface.id.name == name => {
                &interface.body.body
            }
            Declaration::TSTypeAliasDeclaration(alias) if alias.id.name == name => {
                match &alias.type_annotation {
                    TSType::TSTypeLiteral(literal) => &literal.members,
                    _ => continue,
                }
            }
            _ => continue,
        };
        return members
            .iter()
            .filter_map(|member| {
                let TSSignature::TSPropertySignature(property) = member else {
                    return None;
                };
                let name = match &property.key {
                    PropertyKey::StaticIdentifier(id) => id.name.to_string(),
                    PropertyKey::StringLiteral(s) => s.value.to_string(),
                    _ => return None,
                };
                let type_text = property
                    .type_annotation
                    .as_ref()
                    .map(|annotation| {
                        let span = annotation.type_annotation.span();
                        source[span.start as usize..span.end as usize].to_string()
                    })
                    .unwrap_or_default();
                let start = property.span.start;
                let (description, default) = program
                    .comments
                    .iter()
                    .rfind(|comment| comment.span.end <= start)
                    .filter(|comment| {
                        source[comment.span.end as usize..start as usize]
                            .trim()
                            .is_empty()
                    })
                    .map(|comment| {
                        doc(&source[comment.span.start as usize..comment.span.end as usize])
                    })
                    .unwrap_or_default();
                Some(DeclaredParam {
                    name,
                    type_text,
                    optional: property.optional,
                    description,
                    default,
                })
            })
            .collect();
    }
    Vec::new()
}

/// Whether every value of the type is text: `string`, a string literal, or a
/// union of those. A `--param` for such a parameter is kept as given rather
/// than read as JSON, so `--param version=4` stays `"4"`.
pub fn is_string_type(type_text: &str) -> bool {
    let members: Vec<&str> = type_text
        .trim()
        .trim_start_matches('|')
        .split('|')
        .map(str::trim)
        .collect();
    !type_text.trim().is_empty()
        && members.iter().all(|member| {
            *member == "string"
                || [('"', '"'), ('\'', '\''), ('`', '`')]
                    .iter()
                    .any(|(open, close)| {
                        member.len() >= 2 && member.starts_with(*open) && member.ends_with(*close)
                    })
        })
}

/// The names of the parameters `source` declares as text (see
/// [`is_string_type`]).
pub fn string_params(source: &str) -> Vec<String> {
    declared_params(source)
        .into_iter()
        .filter(|param| is_string_type(&param.type_text))
        .map(|param| param.name)
        .collect()
}

/// The name in the first `WorkflowContext<Name>` of the source.
fn context_type_name(source: &str) -> Option<String> {
    let name = extract_context_type(source)?;
    (!name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$'))
    .then(|| name.to_string())
}

/// The text between `WorkflowContext<` and its closing `>`.
fn extract_context_type(source: &str) -> Option<&str> {
    let start = source.find("WorkflowContext<")? + "WorkflowContext<".len();
    let mut depth = 1usize;
    for (offset, ch) in source[start..].char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(source[start..start + offset].trim());
                }
            }
            _ => {}
        }
    }
    None
}

/// A doc comment's text, without the comment markers and tags, and its
/// `@default` value.
fn doc(comment: &str) -> (Option<String>, Option<serde_json::Value>) {
    let body = comment
        .strip_prefix("/**")
        .or_else(|| comment.strip_prefix("/*"))
        .map(|rest| rest.strip_suffix("*/").unwrap_or(rest))
        .or_else(|| comment.strip_prefix("//"))
        .unwrap_or(comment);
    let mut text = Vec::new();
    let mut default = None;
    for line in body
        .lines()
        .map(|line| line.trim().trim_start_matches('*').trim())
        .filter(|line| !line.is_empty())
    {
        if let Some(value) = line.strip_prefix("@default") {
            let value = value.trim();
            default = Some(
                serde_json::from_str(value)
                    .unwrap_or_else(|_| serde_json::Value::from(value.to_string())),
            );
        } else if !line.starts_with('@') {
            text.push(line);
        }
    }
    let text = text.join(" ");
    ((!text.is_empty()).then_some(text), default)
}

/// The modules `source` imports, as written, in source order without
/// repeats: `codemod/workflow`, `./scripts/migrate.ts`, ... Static imports
/// and re-exports only.
pub fn imported_modules(source: &str) -> Vec<String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
    let mut modules: Vec<String> = Vec::new();
    for statement in &parsed.program.body {
        let module = match statement {
            Statement::ImportDeclaration(import) => import.source.value.to_string(),
            Statement::ExportAllDeclaration(export) => export.source.value.to_string(),
            Statement::ExportNamedDeclaration(export) => match &export.source {
                Some(source) => source.value.to_string(),
                None => continue,
            },
            _ => continue,
        };
        if !modules.contains(&module) {
            modules.push(module);
        }
    }
    modules
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_interface_the_context_names() {
        let source = r#"
import type { WorkflowContext } from "codemod/workflow";
interface Options {
  /** Only migrate this package, e.g. "api". */
  package?: string;
  /**
   * Hand leftovers to an agent.
   * @default false
   */
  agent?: boolean | string;
  // A required one.
  level: number;
}
interface Params { ignored: string }
export default async function main({ params }: WorkflowContext<Options>) {}
"#;
        assert_eq!(
            declared_params(source),
            vec![
                DeclaredParam {
                    name: "package".into(),
                    type_text: "string".into(),
                    optional: true,
                    description: Some("Only migrate this package, e.g. \"api\".".into()),
                    default: None,
                },
                DeclaredParam {
                    name: "agent".into(),
                    type_text: "boolean | string".into(),
                    optional: true,
                    description: Some("Hand leftovers to an agent.".into()),
                    default: Some(serde_json::json!(false)),
                },
                DeclaredParam {
                    name: "level".into(),
                    type_text: "number".into(),
                    optional: false,
                    description: Some("A required one.".into()),
                    default: None,
                },
            ]
        );
    }

    #[test]
    fn reads_an_exported_type_alias_named_params() {
        let source =
            "export type Params = { name: string };\nexport default async function main() {}";
        let params = declared_params(source);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "name");
        assert_eq!(params[0].description, None);
    }

    #[test]
    fn a_default_that_is_not_json_is_text() {
        let source = "interface Params {\n  /** Where. @see x\n   * @default src/app */\n  dir?: string;\n}\nexport default async function main() {}";
        let params = declared_params(source);
        assert_eq!(params[0].description.as_deref(), Some("Where. @see x"));
        assert_eq!(params[0].default, Some(serde_json::json!("src/app")));
    }

    #[test]
    fn imports_are_listed_once_in_source_order() {
        let source = r#"
import { step } from "codemod/workflow";
import type { WorkflowContext } from "codemod/workflow";
import replace from "./scripts/replace.ts";
export { x } from "codemod/git";
"#;
        assert_eq!(
            imported_modules(source),
            ["codemod/workflow", "./scripts/replace.ts", "codemod/git"]
        );
    }

    #[test]
    fn string_types_are_text_only() {
        for text in ["string", "\"a\" | \"b\"", "| 'x' | string", "`v${number}`"] {
            assert!(is_string_type(text), "{text}");
        }
        for text in ["", "number", "boolean | string", "string[]", "any"] {
            assert!(!is_string_type(text), "{text}");
        }
        let source = "interface Params {\n  version: string;\n  count?: number;\n  mode?: \"a\" | \"b\";\n  flag: boolean | string;\n}\nexport default async function main() {}";
        assert_eq!(string_params(source), ["version", "mode"]);
    }

    #[test]
    fn nothing_declared_is_no_params() {
        assert!(declared_params("export default async function main() {}").is_empty());
        assert!(declared_params("this is not code {").is_empty());
    }
}
