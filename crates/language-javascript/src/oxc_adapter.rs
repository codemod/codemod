//! OXC integration for parsing and semantic analysis.

use crate::cache::{
    hash_content, ExportedSymbol, FileSymbols, ImportedSymbol, Symbol, SymbolReference,
};
use crate::error::JsSemanticError;
use language_core::{ByteRange, SymbolKind};
use oxc::allocator::Allocator;
use oxc::ast::ast::{
    Declaration, ExportDefaultDeclarationKind, ExportNamedDeclaration, ImportDeclaration,
    ImportDeclarationSpecifier, ModuleDeclaration,
};
use oxc::parser::Parser;
use oxc::semantic::{SemanticBuilder, SymbolFlags};
use oxc::span::{SourceType, Span};
use std::path::Path;

/// Parse a JavaScript/TypeScript file and extract semantic information.
pub fn parse_and_analyze(
    file_path: &Path,
    content: &str,
) -> Result<FileSymbols, JsSemanticError> {
    let allocator = Allocator::default();

    // Determine source type from file extension
    let source_type = SourceType::from_path(file_path).unwrap_or_default();

    // Parse the source
    let parser_return = Parser::new(&allocator, content, source_type).parse();

    if !parser_return.errors.is_empty() {
        let error_messages: Vec<String> = parser_return
            .errors
            .iter()
            .map(|e| e.to_string())
            .collect();
        return Err(JsSemanticError::ParseError {
            path: file_path.to_path_buf(),
            message: error_messages.join("; "),
        });
    }

    let program = parser_return.program;

    // Build semantic information
    let semantic_return = SemanticBuilder::new().build(&program);

    if !semantic_return.errors.is_empty() {
        let error_messages: Vec<String> = semantic_return
            .errors
            .iter()
            .map(|e| e.to_string())
            .collect();
        return Err(JsSemanticError::ParseError {
            path: file_path.to_path_buf(),
            message: error_messages.join("; "),
        });
    }

    let semantic = semantic_return.semantic;
    let scoping = semantic.scoping();

    let mut file_symbols = FileSymbols {
        content_hash: hash_content(content),
        ..Default::default()
    };

    // Extract symbols from the symbol table
    for symbol_id in scoping.symbol_ids() {
        let name = scoping.symbol_name(symbol_id).to_string();
        let flags = scoping.symbol_flags(symbol_id);
        let span = scoping.symbol_span(symbol_id);
        let scope_id = scoping.symbol_scope_id(symbol_id);

        let kind = symbol_flags_to_kind(flags);

        file_symbols.symbols.push(Symbol {
            name,
            kind,
            range: span_to_byte_range(span),
            symbol_id: symbol_id.index() as u32,
            scope_id: scope_id.index() as u32,
        });

        // Extract references for this symbol
        for reference_id in scoping.get_resolved_reference_ids(symbol_id) {
            let reference = scoping.get_reference(*reference_id);
            // Get span from the semantic's reference_span method
            let ref_span = semantic.reference_span(reference);
            file_symbols.references.push(SymbolReference {
                symbol_id: symbol_id.index() as u32,
                range: span_to_byte_range(ref_span),
                is_write: reference.flags().is_write(),
            });
        }
    }

    // Extract imports and exports from the AST
    for stmt in &program.body {
        // Use as_module_declaration() to check if this is a module declaration
        if let Some(module_decl) = stmt.as_module_declaration() {
            match module_decl {
                ModuleDeclaration::ImportDeclaration(import_decl) => {
                    extract_imports(import_decl, &mut file_symbols);
                }
                ModuleDeclaration::ExportNamedDeclaration(export_decl) => {
                    extract_named_exports(export_decl, &mut file_symbols);
                }
                ModuleDeclaration::ExportDefaultDeclaration(export_decl) => {
                    let range = span_to_byte_range(export_decl.span);
                    let local_symbol_id = match &export_decl.declaration {
                        ExportDefaultDeclarationKind::Identifier(id) => {
                            // Find the symbol ID for this identifier
                            file_symbols
                                .symbols
                                .iter()
                                .find(|s| s.name == id.name.as_str())
                                .map(|s| s.symbol_id)
                        }
                        _ => None,
                    };
                    file_symbols.exports.push(ExportedSymbol {
                        name: "default".to_string(),
                        local_symbol_id,
                        range,
                        is_default: true,
                        re_export_from: None,
                    });
                }
                ModuleDeclaration::ExportAllDeclaration(export_all) => {
                    let range = span_to_byte_range(export_all.span);
                    file_symbols.exports.push(ExportedSymbol {
                        name: "*".to_string(),
                        local_symbol_id: None,
                        range,
                        is_default: false,
                        re_export_from: Some(export_all.source.value.to_string()),
                    });
                }
                _ => {}
            }
        }
    }

    Ok(file_symbols)
}

/// Convert OXC Span to ByteRange.
pub fn span_to_byte_range(span: Span) -> ByteRange {
    ByteRange::new(span.start, span.end)
}

/// Convert OXC SymbolFlags to SymbolKind.
fn symbol_flags_to_kind(flags: SymbolFlags) -> SymbolKind {
    if flags.contains(SymbolFlags::Function) {
        SymbolKind::Function
    } else if flags.contains(SymbolFlags::Class) {
        SymbolKind::Class
    } else if flags.contains(SymbolFlags::Interface) {
        SymbolKind::Interface
    } else if flags.contains(SymbolFlags::TypeAlias) {
        SymbolKind::Type
    } else if flags.contains(SymbolFlags::Enum) {
        SymbolKind::Enum
    } else if flags.contains(SymbolFlags::EnumMember) {
        SymbolKind::EnumMember
    } else if flags.contains(SymbolFlags::ConstVariable) {
        SymbolKind::Constant
    } else if flags.contains(SymbolFlags::FunctionScopedVariable)
        || flags.contains(SymbolFlags::BlockScopedVariable)
    {
        SymbolKind::Variable
    } else if flags.contains(SymbolFlags::Import) {
        SymbolKind::Import
    } else if flags.contains(SymbolFlags::TypeParameter) {
        SymbolKind::TypeParameter
    } else {
        SymbolKind::Unknown
    }
}

/// Extract import information from an import declaration.
fn extract_imports(import_decl: &ImportDeclaration, file_symbols: &mut FileSymbols) {
    let module_specifier = import_decl.source.value.to_string();

    if let Some(specifiers) = &import_decl.specifiers {
        for spec in specifiers {
            match spec {
                ImportDeclarationSpecifier::ImportSpecifier(named) => {
                    file_symbols.imports.push(ImportedSymbol {
                        local_name: named.local.name.to_string(),
                        imported_name: Some(named.imported.name().to_string()),
                        module_specifier: module_specifier.clone(),
                        range: span_to_byte_range(named.span),
                        is_default: false,
                        is_namespace: false,
                    });
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(default) => {
                    file_symbols.imports.push(ImportedSymbol {
                        local_name: default.local.name.to_string(),
                        imported_name: None,
                        module_specifier: module_specifier.clone(),
                        range: span_to_byte_range(default.span),
                        is_default: true,
                        is_namespace: false,
                    });
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(ns) => {
                    file_symbols.imports.push(ImportedSymbol {
                        local_name: ns.local.name.to_string(),
                        imported_name: None,
                        module_specifier: module_specifier.clone(),
                        range: span_to_byte_range(ns.span),
                        is_default: false,
                        is_namespace: true,
                    });
                }
            }
        }
    }
}

/// Extract named export information.
fn extract_named_exports(export_decl: &ExportNamedDeclaration, file_symbols: &mut FileSymbols) {
    let range = span_to_byte_range(export_decl.span);
    let re_export_from = export_decl.source.as_ref().map(|s| s.value.to_string());

    // Export specifiers (export { a, b })
    for spec in &export_decl.specifiers {
        let name = spec.exported.name().to_string();
        let local_name = spec.local.name().to_string();

        let local_symbol_id = file_symbols
            .symbols
            .iter()
            .find(|s| s.name == local_name)
            .map(|s| s.symbol_id);

        file_symbols.exports.push(ExportedSymbol {
            name,
            local_symbol_id,
            range,
            is_default: false,
            re_export_from: re_export_from.clone(),
        });
    }

    // Export declaration (export const x = ...)
    if let Some(decl) = &export_decl.declaration {
        match decl {
            Declaration::VariableDeclaration(var_decl) => {
                for declarator in &var_decl.declarations {
                    if let Some(name) = declarator.id.get_identifier_name() {
                        let name_str = name.to_string();
                        let local_symbol_id = file_symbols
                            .symbols
                            .iter()
                            .find(|s| s.name == name_str)
                            .map(|s| s.symbol_id);

                        file_symbols.exports.push(ExportedSymbol {
                            name: name_str,
                            local_symbol_id,
                            range,
                            is_default: false,
                            re_export_from: None,
                        });
                    }
                }
            }
            Declaration::FunctionDeclaration(func_decl) => {
                if let Some(id) = &func_decl.id {
                    let name = id.name.to_string();
                    let local_symbol_id = file_symbols
                        .symbols
                        .iter()
                        .find(|s| s.name == name)
                        .map(|s| s.symbol_id);

                    file_symbols.exports.push(ExportedSymbol {
                        name,
                        local_symbol_id,
                        range,
                        is_default: false,
                        re_export_from: None,
                    });
                }
            }
            Declaration::ClassDeclaration(class_decl) => {
                if let Some(id) = &class_decl.id {
                    let name = id.name.to_string();
                    let local_symbol_id = file_symbols
                        .symbols
                        .iter()
                        .find(|s| s.name == name)
                        .map(|s| s.symbol_id);

                    file_symbols.exports.push(ExportedSymbol {
                        name,
                        local_symbol_id,
                        range,
                        is_default: false,
                        re_export_from: None,
                    });
                }
            }
            Declaration::TSTypeAliasDeclaration(type_alias) => {
                let name = type_alias.id.name.to_string();
                let local_symbol_id = file_symbols
                    .symbols
                    .iter()
                    .find(|s| s.name == name)
                    .map(|s| s.symbol_id);

                file_symbols.exports.push(ExportedSymbol {
                    name,
                    local_symbol_id,
                    range,
                    is_default: false,
                    re_export_from: None,
                });
            }
            Declaration::TSInterfaceDeclaration(interface_decl) => {
                let name = interface_decl.id.name.to_string();
                let local_symbol_id = file_symbols
                    .symbols
                    .iter()
                    .find(|s| s.name == name)
                    .map(|s| s.symbol_id);

                file_symbols.exports.push(ExportedSymbol {
                    name,
                    local_symbol_id,
                    range,
                    is_default: false,
                    re_export_from: None,
                });
            }
            Declaration::TSEnumDeclaration(enum_decl) => {
                let name = enum_decl.id.name.to_string();
                let local_symbol_id = file_symbols
                    .symbols
                    .iter()
                    .find(|s| s.name == name)
                    .map(|s| s.symbol_id);

                file_symbols.exports.push(ExportedSymbol {
                    name,
                    local_symbol_id,
                    range,
                    is_default: false,
                    re_export_from: None,
                });
            }
            _ => {}
        }
    }
}

/// Find the symbol at a given byte range using the semantic model.
pub fn find_symbol_at_range(
    file_symbols: &FileSymbols,
    range: ByteRange,
) -> Option<&Symbol> {
    // First try to find an exact match or containing symbol
    file_symbols.find_symbol_at(range)
}

/// Find references in a file symbols to a given symbol ID.
#[allow(dead_code)]
pub fn find_references_in_file(
    file_symbols: &FileSymbols,
    symbol_id: u32,
) -> Vec<&SymbolReference> {
    file_symbols.find_references_to(symbol_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_parse_simple_function() {
        let content = r#"
function greet(name: string): string {
    return "Hello, " + name;
}
        "#;

        let result = parse_and_analyze(Path::new("test.ts"), content);
        assert!(result.is_ok());

        let symbols = result.unwrap();
        assert!(!symbols.symbols.is_empty());

        // Should have the function and parameter
        let func = symbols.symbols.iter().find(|s| s.name == "greet");
        assert!(func.is_some());
        assert_eq!(func.unwrap().kind, SymbolKind::Function);

        let param = symbols.symbols.iter().find(|s| s.name == "name");
        assert!(param.is_some());
    }

    #[test]
    fn test_parse_imports() {
        let content = r#"
import React from 'react';
import { useState, useEffect } from 'react';
import * as utils from './utils';
        "#;

        let result = parse_and_analyze(Path::new("test.tsx"), content);
        assert!(result.is_ok());

        let symbols = result.unwrap();
        assert_eq!(symbols.imports.len(), 4);

        // Default import
        let react_import = symbols.imports.iter().find(|i| i.local_name == "React");
        assert!(react_import.is_some());
        assert!(react_import.unwrap().is_default);

        // Named imports
        let use_state = symbols.imports.iter().find(|i| i.local_name == "useState");
        assert!(use_state.is_some());
        assert!(!use_state.unwrap().is_default);

        // Namespace import
        let utils_import = symbols.imports.iter().find(|i| i.local_name == "utils");
        assert!(utils_import.is_some());
        assert!(utils_import.unwrap().is_namespace);
    }

    #[test]
    fn test_parse_exports() {
        let content = r#"
export const FOO = 'foo';
export function bar() {}
export default class MyClass {}
export { something } from './other';
        "#;

        let result = parse_and_analyze(Path::new("test.ts"), content);
        assert!(result.is_ok());

        let symbols = result.unwrap();
        assert!(!symbols.exports.is_empty());

        // Named export
        let foo_export = symbols.exports.iter().find(|e| e.name == "FOO");
        assert!(foo_export.is_some());
        assert!(!foo_export.unwrap().is_default);

        // Default export
        let default_export = symbols.exports.iter().find(|e| e.is_default);
        assert!(default_export.is_some());

        // Re-export
        let re_export = symbols.exports.iter().find(|e| e.name == "something");
        assert!(re_export.is_some());
        assert!(re_export.unwrap().re_export_from.is_some());
    }

    #[test]
    fn test_parse_references() {
        let content = r#"
const x = 1;
const y = x + 2;
console.log(x, y);
        "#;

        let result = parse_and_analyze(Path::new("test.ts"), content);
        assert!(result.is_ok());

        let symbols = result.unwrap();
        
        // Should have references to x
        let x_symbol = symbols.symbols.iter().find(|s| s.name == "x");
        assert!(x_symbol.is_some());
        
        let x_refs = symbols.find_references_to(x_symbol.unwrap().symbol_id);
        assert!(!x_refs.is_empty());
    }

    #[test]
    fn test_parse_error_handling() {
        let content = r#"
function broken( {
    // Missing closing paren
}
        "#;

        let result = parse_and_analyze(Path::new("test.ts"), content);
        assert!(result.is_err());
        
        match result {
            Err(JsSemanticError::ParseError { path, message }) => {
                assert_eq!(path, PathBuf::from("test.ts"));
                assert!(!message.is_empty());
            }
            _ => panic!("Expected ParseError"),
        }
    }
}
