//! Definitions, references and types, answered by rust-analyzer.

use crate::facts::range_of;
use crate::workspace::RaWorkspace;
use language_core::{
    ByteRange, DefinitionKind, DefinitionResult, FileReferences, ReferencesResult, SymbolKind,
    SymbolLocation,
};
use ra_ap_hir::{HirDisplay, Semantics};
use ra_ap_ide::{FilePosition, FindAllRefsConfig, GotoDefinitionConfig, NavigationTarget};
use ra_ap_ide_db::{ra_fixture::RaFixtureConfig, RootDatabase};
use ra_ap_syntax::{ast, AstNode, NodeOrToken, SyntaxNode, TextRange, TextSize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn fixture_config<'a>() -> RaFixtureConfig<'a> {
    RaFixtureConfig {
        disable_ra_fixture: true,
        ..RaFixtureConfig::default()
    }
}

fn symbol_kind(kind: Option<ra_ap_ide::SymbolKind>) -> SymbolKind {
    use ra_ap_ide::SymbolKind as Ra;
    match kind {
        Some(Ra::Function) => SymbolKind::Function,
        Some(Ra::Method) => SymbolKind::Method,
        Some(Ra::Struct | Ra::Union) => SymbolKind::Class,
        Some(Ra::Enum) => SymbolKind::Enum,
        Some(Ra::Variant) => SymbolKind::EnumMember,
        Some(Ra::Trait) => SymbolKind::Interface,
        Some(Ra::TypeAlias) => SymbolKind::Type,
        Some(Ra::Field) => SymbolKind::Property,
        Some(Ra::Module | Ra::CrateRoot) => SymbolKind::Namespace,
        Some(Ra::Const | Ra::Static) => SymbolKind::Constant,
        Some(Ra::ValueParam | Ra::SelfParam) => SymbolKind::Parameter,
        Some(Ra::TypeParam | Ra::ConstParam | Ra::LifetimeParam) => SymbolKind::TypeParameter,
        _ => SymbolKind::Variable,
    }
}

/// The offset to ask about for a node's range: its first identifier, so
/// `helper(1)`, `self.total` and `x` all ask about a name.
fn query_offset(root: &SyntaxNode, range: TextRange) -> TextSize {
    let ident = root
        .covering_element(range)
        .into_node()
        .into_iter()
        .flat_map(|node| node.descendants_with_tokens().collect::<Vec<_>>())
        .filter_map(NodeOrToken::into_token)
        .filter(|t| range.contains_range(t.text_range()))
        .find(|t| {
            t.kind() == ra_ap_syntax::SyntaxKind::IDENT || t.kind() == ra_ap_syntax::T![self]
        });
    ident
        .map(|t| t.text_range().start())
        .unwrap_or(range.start())
}

fn location(ws: &RaWorkspace, nav: &NavigationTarget) -> Option<(PathBuf, String, SymbolLocation)> {
    let path = ws.path(nav.file_id)?;
    let content = ws.text(&path)?.to_string();
    let range = nav.focus_range.unwrap_or(nav.full_range);
    let location = SymbolLocation::new(
        path.clone(),
        range_of(range),
        symbol_kind(nav.kind),
        nav.name.as_str().to_string(),
    );
    Some((path, content, location))
}

/// Where the name at `range` of `path` is declared.
pub(crate) fn definition(
    ws: &RaWorkspace,
    path: &Path,
    range: ByteRange,
) -> Option<DefinitionResult> {
    let file_id = ws.file_id(path)?;
    let root = ws.ask(|a| a.parse(file_id)).syntax().clone();
    let range = TextRange::new(range.start.into(), range.end.into());
    let offset = query_offset(&root, range);
    let config = GotoDefinitionConfig {
        ra_fixture: fixture_config(),
    };
    let targets = ws
        .ask(|a| a.goto_definition(FilePosition { file_id, offset }, &config))?
        .info;
    // Prefer a declaration in the workspace over one in a dependency.
    let nav = targets
        .iter()
        .find(|nav| ws.path(nav.file_id).is_some_and(|p| ws.is_local(&p)))
        .or_else(|| targets.first())?;
    let (target, content, location) = location(ws, nav)?;
    let kind = if target == path {
        DefinitionKind::Local
    } else {
        DefinitionKind::External
    };
    Some(DefinitionResult {
        location,
        content,
        kind,
        export_name: None,
    })
}

/// Every use of the name at `range` of `path`, in the workspace's files.
pub(crate) fn references(ws: &RaWorkspace, path: &Path, range: ByteRange) -> ReferencesResult {
    let mut result = ReferencesResult::new();
    let Some(file_id) = ws.file_id(path) else {
        return result;
    };
    let root = ws.ask(|a| a.parse(file_id)).syntax().clone();
    let range = TextRange::new(range.start.into(), range.end.into());
    let offset = query_offset(&root, range);
    let config = FindAllRefsConfig {
        search_scope: None,
        ra_fixture: fixture_config(),
        exclude_imports: false,
        exclude_tests: false,
    };
    let Some(found) = ws.ask(|a| a.find_all_refs(FilePosition { file_id, offset }, &config)) else {
        return result;
    };
    let mut by_file: BTreeMap<PathBuf, Vec<SymbolLocation>> = BTreeMap::new();
    for search in found {
        let name = search
            .declaration
            .as_ref()
            .map(|d| d.nav.name.as_str().to_string())
            .unwrap_or_default();
        let kind = symbol_kind(search.declaration.as_ref().and_then(|d| d.nav.kind));
        for (file, ranges) in search.references {
            let Some(file_path) = ws.path(file) else {
                continue;
            };
            if !ws.is_local(&file_path) {
                continue;
            }
            for (range, _) in ranges {
                by_file
                    .entry(file_path.clone())
                    .or_default()
                    .push(SymbolLocation::new(
                        file_path.clone(),
                        range_of(range),
                        kind,
                        name.clone(),
                    ));
            }
        }
    }
    for (file_path, mut locations) in by_file {
        locations.sort_by_key(|l| (l.range.start, l.range.end));
        locations.dedup_by_key(|l| (l.range.start, l.range.end));
        let content = ws
            .text(&file_path)
            .map(|t| t.to_string())
            .unwrap_or_default();
        result.add_file(FileReferences::new(file_path, content, locations));
    }
    result
}

/// The type of the expression, pattern or name at `range`, as
/// rust-analyzer infers it (`Vec<String>`, `Option<&User>`); for a
/// function, its signature.
pub(crate) fn type_of(ws: &RaWorkspace, path: &Path, range: ByteRange) -> Option<String> {
    let file_id = ws.file_id(path)?;
    ws.with_db(|db| type_in(db, file_id, range))
}

fn type_in(db: &RootDatabase, file_id: ra_ap_ide::FileId, range: ByteRange) -> Option<String> {
    let sema = Semantics::new(db);
    let source = sema.parse_guess_edition(file_id);
    let range = TextRange::new(range.start.into(), range.end.into());
    let covering = match source.syntax().covering_element(range) {
        NodeOrToken::Node(node) => node,
        NodeOrToken::Token(token) => token.parent()?,
    };
    let module = sema.file_to_module_def(file_id)?;
    let target = module.krate(db).to_display_target(db);
    for node in covering.ancestors() {
        if node.text_range() != range && !range.contains_range(node.text_range()) {
            break;
        }
        if let Some(expr) = ast::Expr::cast(node.clone()) {
            if let Some(info) = sema.type_of_expr(&expr) {
                return Some(
                    info.original
                        .display_truncated(db, None, target)
                        .to_string(),
                );
            }
        }
        if let Some(pat) = ast::IdentPat::cast(node.clone()) {
            if let Some(ty) = sema.type_of_binding_in_pat(&pat) {
                return Some(ty.display_truncated(db, None, target).to_string());
            }
        }
        if let Some(pat) = ast::Pat::cast(node.clone()) {
            if let Some(info) = sema.type_of_pat(&pat) {
                return Some(
                    info.original
                        .display_truncated(db, None, target)
                        .to_string(),
                );
            }
        }
        if let Some(param) = ast::SelfParam::cast(node.clone()) {
            if let Some(ty) = sema.type_of_self(&param) {
                return Some(ty.display_truncated(db, None, target).to_string());
            }
        }
        if let Some(function) = ast::Fn::cast(node.clone()) {
            let def = sema.to_def(&function)?;
            return Some(def.display(db, target).to_string());
        }
        if let Some(name) = ast::Name::cast(node.clone()) {
            // The name of a `let`, a parameter or a function.
            if let Some(parent) = name.syntax().parent() {
                if let Some(pat) = ast::IdentPat::cast(parent.clone()) {
                    if let Some(ty) = sema.type_of_binding_in_pat(&pat) {
                        return Some(ty.display_truncated(db, None, target).to_string());
                    }
                }
                if let Some(function) = ast::Fn::cast(parent) {
                    let def = sema.to_def(&function)?;
                    return Some(def.display(db, target).to_string());
                }
            }
        }
    }
    None
}
