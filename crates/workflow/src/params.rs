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
    /// Its doc comment, without the comment markers.
    pub description: Option<String>,
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
                let description = program
                    .comments
                    .iter()
                    .rfind(|comment| comment.span.end <= start)
                    .filter(|comment| {
                        source[comment.span.end as usize..start as usize]
                            .trim()
                            .is_empty()
                    })
                    .map(|comment| {
                        clean_comment(
                            &source[comment.span.start as usize..comment.span.end as usize],
                        )
                    })
                    .filter(|text| !text.is_empty());
                Some(DeclaredParam {
                    name,
                    type_text,
                    optional: property.optional,
                    description,
                })
            })
            .collect();
    }
    Vec::new()
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

fn clean_comment(comment: &str) -> String {
    let body = comment
        .strip_prefix("/**")
        .or_else(|| comment.strip_prefix("/*"))
        .map(|rest| rest.strip_suffix("*/").unwrap_or(rest))
        .or_else(|| comment.strip_prefix("//"))
        .unwrap_or(comment);
    body.lines()
        .map(|line| line.trim().trim_start_matches('*').trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
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
                },
                DeclaredParam {
                    name: "agent".into(),
                    type_text: "boolean | string".into(),
                    optional: true,
                    description: Some("Hand leftovers to an agent.".into()),
                },
                DeclaredParam {
                    name: "level".into(),
                    type_text: "number".into(),
                    optional: false,
                    description: Some("A required one.".into()),
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
    fn nothing_declared_is_no_params() {
        assert!(declared_params("export default async function main() {}").is_empty());
        assert!(declared_params("this is not code {").is_empty());
    }
}
