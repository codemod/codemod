//! Rust module paths: `crate::billing::invoice` to the file that holds the
//! module, a file back to the path that reaches it, and the `use` items and
//! `mod` declarations that refer to a module.

use crate::facts::range_of;
use crate::workspace::RaWorkspace;
use language_core::{ImportKind, Importer, ImportersFile, ImportersResult};
use ra_ap_hir::{Module, Semantics};
use ra_ap_ide::{FilePosition, FindAllRefsConfig};
use ra_ap_ide_db::{ra_fixture::RaFixtureConfig, RootDatabase};
use ra_ap_syntax::{
    ast::{self, HasName, HasVisibility},
    AstNode,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The file a module's items are written in (for `mod a { … }`, the file
/// it is written inside).
fn module_file(ws: &RaWorkspace, db: &RootDatabase, module: Module) -> Option<PathBuf> {
    let file = module
        .definition_source_file_id(db)
        .original_file(db)
        .file_id(db);
    ws.path(file)
}

fn child(db: &RootDatabase, module: Module, name: &str) -> Option<Module> {
    module
        .children(db)
        .find(|m| m.name(db).is_some_and(|n| n.as_str() == name))
}

/// The module `specifier` names, written in a file of the module `from`:
/// `crate::a::b`, `self::a`, `super::a`, `a::b` (a child of `from`), or
/// `other_crate::a` (a crate `from`'s crate depends on).
fn resolve(db: &RootDatabase, from: Module, specifier: &str) -> Option<Module> {
    let specifier = specifier.trim().trim_start_matches("::");
    let mut segments = specifier.split("::").filter(|s| !s.is_empty()).peekable();
    let mut module = match *segments.peek()? {
        "crate" => {
            segments.next();
            from.crate_root(db)
        }
        "self" => {
            segments.next();
            from
        }
        "super" => from,
        first => match child(db, from, first) {
            Some(_) => from,
            None => {
                // `serde::de`: a dependency's root.
                let dependency = from
                    .krate(db)
                    .dependencies(db)
                    .into_iter()
                    .find(|d| d.name.as_str() == first)?;
                segments.next();
                dependency.krate.root_module(db)
            }
        },
    };
    for segment in segments {
        module = if segment == "super" {
            module.parent(db)?
        } else {
            child(db, module, segment)?
        };
    }
    Some(module)
}

/// The workspace file the module path `specifier` (written in `from`)
/// reaches.
pub(crate) fn resolve_module(ws: &RaWorkspace, from: &Path, specifier: &str) -> Option<PathBuf> {
    let file_id = ws.file_id(from)?;
    ws.with_db(|db| {
        let module = Semantics::new(db).file_to_module_def(file_id)?;
        module_file(ws, db, resolve(db, module, specifier)?)
    })
}

/// The module of a file, if the file is a module (`src/billing/mod.rs`,
/// `src/billing.rs`), not only included in one.
fn file_module(ws: &RaWorkspace, db: &RootDatabase, file: &Path) -> Option<Module> {
    let file_id = ws.file_id(file)?;
    Semantics::new(db).file_to_module_def(file_id)
}

/// A path that reaches the module of `target` from `from`: `crate::a::b`
/// within one crate, `other_crate::a::b` across crates.
pub(crate) fn specifier_for(ws: &RaWorkspace, from: &Path, target: &Path) -> Option<String> {
    ws.with_db(|db| specifier_in(ws, db, from, target))
}

fn specifier_in(ws: &RaWorkspace, db: &RootDatabase, from: &Path, target: &Path) -> Option<String> {
    let target = file_module(ws, db, target)?;
    let from = file_module(ws, db, from)?;
    let names: Vec<String> = target
        .path_to_root(db)
        .into_iter()
        .rev()
        .filter_map(|m| m.name(db).map(|n| n.as_str().to_string()))
        .collect();
    let root = if target.krate(db) == from.krate(db) {
        "crate".to_string()
    } else {
        let name = target.krate(db).display_name(db)?.to_string();
        name.replace('-', "_")
    };
    let mut parts = vec![root];
    parts.extend(names);
    Some(parts.join("::"))
}

/// The `use` items and `mod` declarations in the workspace that refer to
/// the module of `target`.
pub(crate) fn importers(ws: &RaWorkspace, target: &Path) -> ImportersResult {
    let mut result = ImportersResult::default();
    // Ask for references at the module's declaration (`mod billing;`).
    let declared = ws.with_db(|db| {
        let module = file_module(ws, db, target)?;
        let declaration = module.declaration_source_range(db)?;
        let file = declaration.file_id.original_file(db).file_id(db);
        Some((
            file,
            declaration.value,
            module.name(db)?.as_str().to_string(),
        ))
    });
    let Some((declared_in, declaration, name)) = declared else {
        return result;
    };
    let source = ws.ask(|a| a.parse(declared_in));
    let Some(mod_item) = source
        .syntax()
        .covering_element(declaration)
        .ancestors()
        .find_map(ast::Module::cast)
    else {
        return result;
    };
    let Some(name_node) = mod_item.name() else {
        return result;
    };
    let config = FindAllRefsConfig {
        search_scope: None,
        ra_fixture: RaFixtureConfig {
            disable_ra_fixture: true,
            ..RaFixtureConfig::default()
        },
        exclude_imports: false,
        exclude_tests: false,
    };
    let position = FilePosition {
        file_id: declared_in,
        offset: name_node.syntax().text_range().start(),
    };
    let found = ws
        .ask(|a| a.find_all_refs(position, &config))
        .unwrap_or_default();
    let mut by_file: BTreeMap<PathBuf, Vec<Importer>> = BTreeMap::new();
    if let Some(declaring) = ws.path(declared_in) {
        by_file.entry(declaring).or_default().push(Importer {
            range: range_of(mod_item.syntax().text_range()),
            kind: ImportKind::Import,
            specifier: format!("mod {name}"),
        });
    }
    for search in found {
        for (file, ranges) in search.references {
            let Some(path) = ws.path(file) else {
                continue;
            };
            if !ws.is_local(&path) {
                continue;
            }
            let parsed = ws.ask(|a| a.parse(file));
            for (range, _) in ranges {
                let Some(use_item) = parsed
                    .syntax()
                    .covering_element(range)
                    .ancestors()
                    .find_map(ast::Use::cast)
                else {
                    continue;
                };
                let kind = if use_item.visibility().is_some() {
                    ImportKind::Export
                } else {
                    ImportKind::Import
                };
                let specifier = use_item
                    .use_tree()
                    .map(|t| t.syntax().text().to_string())
                    .unwrap_or_default();
                by_file.entry(path.clone()).or_default().push(Importer {
                    range: range_of(use_item.syntax().text_range()),
                    kind,
                    specifier,
                });
            }
        }
    }
    for (file_path, mut importers) in by_file {
        importers.sort_by_key(|i| (i.range.start, i.range.end));
        importers.dedup_by_key(|i| (i.range.start, i.range.end));
        let content = ws
            .text(&file_path)
            .map(|t| t.to_string())
            .unwrap_or_default();
        result.files.push(ImportersFile {
            file_path,
            content,
            importers,
        });
    }
    result
}
