//! The `jssg()` split. Every `jssg({ ..., transform })` call in a workflow
//! module becomes an artifact: the transform's own source text, with the
//! import declarations it uses, bundled into an ES module whose default
//! export is the transform. The call is rewritten so `transform` is an
//! `ArtifactRef` (`{ name, hash }`); no function is ever serialized.
//!
//! Supported subset: a transform may use its own parameters and locals,
//! globals, and bindings the module imports from other modules (those are
//! bundled). It may not use anything else declared in the workflow module;
//! such captures are rejected with the source position. `name` must be a
//! string literal; `transform` must be a method, a function or arrow
//! expression, or an imported binding.
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use oxc::{
    allocator::Allocator,
    ast::{
        ast::{
            Argument, Expression, ObjectExpression, ObjectProperty, ObjectPropertyKind,
            PropertyKey, PropertyKind, Statement,
        },
        AstKind,
    },
    parser::Parser,
    semantic::{Semantic, SemanticBuilder, SymbolId},
    span::{GetSpan, SourceType, Span},
};

use crate::build::{bundle::bundle_artifact, sha256, BuildError};

const ORCHESTRATION_PACKAGE: &str = "@codemod.com/orchestration";

/// One bundled transform. `source` is what a runtime executes; `hash` identifies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JssgArtifact {
    pub name: String,
    pub hash: String,
    pub source: String,
    /// The module the transform was extracted from, for messages only.
    pub origin: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltModule {
    /// The module with every inline transform replaced by its `ArtifactRef`.
    pub source: String,
    pub artifacts: Vec<JssgArtifact>,
    /// Every file bundled into the artifacts, the module itself included
    /// (for build caches). Sorted.
    pub inputs: Vec<PathBuf>,
}

/// Read and split a module on disk.
pub async fn build_file(file: &Path) -> Result<BuiltModule, BuildError> {
    let source = std::fs::read_to_string(file).map_err(|error| BuildError {
        file: file.to_path_buf(),
        position: None,
        message: error.to_string(),
    })?;
    build_module(&source, file).await
}

/// Split one module. `file` must be absolute: the transforms' relative
/// imports resolve from its directory.
pub async fn build_module(source: &str, file: &Path) -> Result<BuiltModule, BuildError> {
    // The AST is not `Send`, so the analysis finishes before any bundling.
    let plan = analyze(source, file)?;
    let mut edits = plan.removed_imports;
    let mut artifacts = Vec::new();
    let mut inputs = BTreeSet::new();
    for extraction in plan.extractions {
        let bundled = bundle_artifact(&extraction.entry, file).await?;
        inputs.extend(bundled.inputs);
        let hash = sha256(&[bundled.code.as_bytes()]);
        edits.push(Edit {
            span: extraction.property,
            text: format!(
                "transform: {{\"name\":{},\"hash\":{}}}",
                json_string(&extraction.name),
                json_string(&hash)
            ),
        });
        artifacts.push(JssgArtifact {
            name: extraction.name,
            hash,
            source: bundled.code,
            origin: file.to_path_buf(),
        });
    }
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.span.start));
    let mut rewritten = source.to_owned();
    for edit in edits {
        rewritten.replace_range(edit.span.start as usize..edit.span.end as usize, &edit.text);
    }
    Ok(BuiltModule {
        source: rewritten,
        artifacts,
        inputs: inputs.into_iter().collect(),
    })
}

struct Edit {
    span: Span,
    text: String,
}

struct Extraction {
    name: String,
    /// The virtual entry module: the imports the transform uses, then
    /// `export default <transform>`.
    entry: String,
    property: Span,
}

struct Plan {
    extractions: Vec<Extraction>,
    removed_imports: Vec<Edit>,
}

struct Import {
    span: Span,
    source: String,
    symbols: Vec<SymbolId>,
}

struct Module<'s, 'a> {
    source: &'s str,
    file: &'s Path,
    semantic: Semantic<'a>,
    imports: Vec<Import>,
}

fn analyze(source: &str, file: &Path) -> Result<Plan, BuildError> {
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(file).unwrap_or_else(|_| SourceType::ts());
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if let Some(error) = parsed.errors.first() {
        let start = error
            .labels
            .as_ref()
            .and_then(|labels| labels.first())
            .map(|label| label.offset() as u32);
        return Err(BuildError {
            file: file.to_path_buf(),
            position: start.map(|start| position(source, start)),
            message: error.message.to_string(),
        });
    }
    let program = allocator.alloc(parsed.program);
    let semantic = SemanticBuilder::new().build(program).semantic;
    let imports = program
        .body
        .iter()
        .filter_map(|statement| match statement {
            Statement::ImportDeclaration(declaration) => Some(Import {
                span: declaration.span,
                source: declaration.source.value.to_string(),
                symbols: declaration
                    .specifiers
                    .iter()
                    .flatten()
                    .map(|specifier| specifier.local().symbol_id())
                    .collect(),
            }),
            _ => None,
        })
        .collect();
    let module = Module {
        source,
        file,
        semantic,
        imports,
    };

    let mut extractions = Vec::new();
    for node in module.semantic.nodes().iter() {
        let AstKind::CallExpression(call) = node.kind() else {
            continue;
        };
        if !matches!(&call.callee, Expression::Identifier(callee) if callee.name == "jssg") {
            continue;
        }
        let Some(Argument::ObjectExpression(argument)) = call.arguments.first() else {
            return Err(module.fail(
                call.span,
                "jssg() must be called with an object literal so its transform can be extracted",
            ));
        };
        let Some(name) = literal_property(argument, "name") else {
            return Err(module.fail(argument.span, "jssg name must be a string literal"));
        };
        let Some(property) = find_property(argument, "transform") else {
            return Err(module.fail(argument.span, &format!("jssg '{name}' has no transform")));
        };
        let span = property.span;
        let entry = module.transform_entry(property, &name)?;
        extractions.push(Extraction {
            name,
            entry,
            property: span,
        });
    }
    let extracted: Vec<Span> = extractions.iter().map(|e| e.property).collect();
    let removed_imports = module.imports_only_transforms_use(&extracted);
    Ok(Plan {
        extractions,
        removed_imports,
    })
}

impl Module<'_, '_> {
    fn fail(&self, span: Span, message: &str) -> BuildError {
        BuildError {
            file: self.file.to_path_buf(),
            position: Some(position(self.source, span.start)),
            message: message.to_owned(),
        }
    }

    fn text(&self, span: Span) -> &str {
        &self.source[span.start as usize..span.end as usize]
    }

    fn transform_entry(&self, property: &ObjectProperty, name: &str) -> Result<String, BuildError> {
        if property.kind != PropertyKind::Init {
            return Err(self.fail(
                property.span,
                &format!("jssg '{name}' transform must be a method or property"),
            ));
        }
        let (body, range) = if property.method {
            let Expression::FunctionExpression(function) = &property.value else {
                unreachable!("an object method's value is a function");
            };
            if function.generator {
                return Err(self.fail(
                    property.span,
                    &format!("jssg '{name}' transform cannot be a generator"),
                ));
            }
            if function.body.is_none() {
                return Err(self.fail(
                    property.span,
                    &format!("jssg '{name}' transform has no body"),
                ));
            }
            let signature = Span::new(property.key.span().end, property.span.end);
            let body = format!(
                "export default {}function transform{}",
                if function.r#async { "async " } else { "" },
                self.text(signature)
            );
            (body, property.span)
        } else {
            match &property.value {
                Expression::FunctionExpression(_) | Expression::ArrowFunctionExpression(_) => {
                    let span = property.value.span();
                    (format!("export default ({});", self.text(span)), span)
                }
                Expression::Identifier(identifier) => (
                    format!("export default {};", identifier.name),
                    identifier.span,
                ),
                value => {
                    return Err(self.fail(
                        value.span(),
                        &format!("jssg '{name}' transform must be a method, a function or arrow expression, or an imported binding"),
                    ))
                }
            }
        };

        let mut imports = BTreeSet::new();
        for (symbol, reference) in self.module_references(range) {
            if let Some(index) = self
                .imports
                .iter()
                .position(|i| i.symbols.contains(&symbol))
            {
                let import = &self.imports[index];
                if import.source == ORCHESTRATION_PACKAGE
                    || import
                        .source
                        .starts_with(&format!("{ORCHESTRATION_PACKAGE}/"))
                {
                    return Err(self.fail(
                        reference,
                        &format!(
                            "jssg '{name}' transform uses '{}' from {ORCHESTRATION_PACKAGE}; the orchestration runtime is not available inside a transform",
                            self.symbol_name(symbol)
                        ),
                    ));
                }
                imports.insert(index);
                continue;
            }
            let scoping = self.semantic.scoping();
            if scoping.symbol_flags(symbol).is_value() {
                let identifier = self.symbol_name(symbol);
                let (line, _) = position(self.source, scoping.symbol_span(symbol).start);
                return Err(self.fail(
                    reference,
                    &format!(
                        "jssg '{name}' transform uses '{identifier}', declared in the workflow module at line {line}; a transform can only use its own code, globals, and imported bindings. Move '{identifier}' into a module and import it, or pass it through invocation input"
                    ),
                ));
            }
        }
        let imports: Vec<&str> = imports
            .into_iter()
            .map(|index| self.text(self.imports[index].span))
            .collect();
        Ok(format!("{}\n{body}\n", imports.join("\n")))
    }

    fn symbol_name(&self, symbol: SymbolId) -> &str {
        self.semantic.scoping().symbol_name(symbol)
    }

    /// Module-level bindings referenced as values inside `range`, each with
    /// its first reference, in source order. Names bound inside the
    /// transform resolve to their own scopes and never appear here.
    fn module_references(&self, range: Span) -> Vec<(SymbolId, Span)> {
        let scoping = self.semantic.scoping();
        let mut found: Vec<(SymbolId, Span)> = scoping
            .iter_bindings_in(scoping.root_scope_id())
            .filter_map(|symbol| {
                scoping
                    .get_resolved_references(symbol)
                    .filter(|reference| reference.flags().is_value())
                    .map(|reference| self.semantic.nodes().get_node(reference.node_id()).span())
                    .filter(|span| contains(range, *span))
                    .min_by_key(|span| span.start)
                    .map(|span| (symbol, span))
            })
            .collect();
        found.sort_by_key(|(_, span)| span.start);
        found
    }

    /// Imports used only inside extracted transforms live in the artifacts
    /// now. Keeping them would make the runner evaluate transform modules
    /// (and their `codemod:*` imports, which only the runtime resolves).
    fn imports_only_transforms_use(&self, extracted: &[Span]) -> Vec<Edit> {
        let scoping = self.semantic.scoping();
        let inside = |span: Span| extracted.iter().any(|range| contains(*range, span));
        self.imports
            .iter()
            .filter(|import| {
                let spans: Vec<Span> = import
                    .symbols
                    .iter()
                    .flat_map(|symbol| scoping.get_resolved_references(*symbol))
                    .map(|reference| self.semantic.nodes().get_node(reference.node_id()).span())
                    .collect();
                !import.symbols.is_empty()
                    && spans.iter().all(|span| inside(*span))
                    && !spans.is_empty()
            })
            .map(|import| Edit {
                span: import.span,
                text: String::new(),
            })
            .collect()
    }
}

fn contains(range: Span, span: Span) -> bool {
    span.start >= range.start && span.start < range.end
}

fn property_name<'a>(property: &'a ObjectProperty) -> Option<&'a str> {
    if property.computed {
        return None;
    }
    match &property.key {
        PropertyKey::StaticIdentifier(identifier) => Some(identifier.name.as_str()),
        PropertyKey::StringLiteral(literal) => Some(literal.value.as_str()),
        _ => None,
    }
}

fn find_property<'o, 'a>(
    object: &'o ObjectExpression<'a>,
    key: &str,
) -> Option<&'o ObjectProperty<'a>> {
    object
        .properties
        .iter()
        .find_map(|property| match property {
            ObjectPropertyKind::ObjectProperty(property)
                if property_name(property) == Some(key) =>
            {
                Some(&**property)
            }
            _ => None,
        })
}

fn literal_property(object: &ObjectExpression, key: &str) -> Option<String> {
    let property = find_property(object, key)?;
    if property.kind != PropertyKind::Init || property.method || property.shorthand {
        return None;
    }
    match &property.value {
        Expression::StringLiteral(literal) => Some(literal.value.to_string()),
        Expression::TemplateLiteral(template) if template.expressions.is_empty() => template
            .quasis
            .first()
            .and_then(|quasi| quasi.value.cooked)
            .map(|cooked| cooked.to_string()),
        _ => None,
    }
}

/// 1-based line and column of a byte offset; columns count UTF-16 code units.
pub(crate) fn position(source: &str, offset: u32) -> (usize, usize) {
    let before = &source[..offset as usize];
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    let line = before.matches('\n').count() + 1;
    let column = before[line_start..].encode_utf16().count() + 1;
    (line, column)
}

fn json_string(text: &str) -> String {
    serde_json::to_string(text).expect("a string serializes")
}
