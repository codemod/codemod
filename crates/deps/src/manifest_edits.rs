//! Manifest edits over a [`Project`]: set, add, remove and rename
//! declarations where they are declared, following `catalog:` refs into
//! their catalogs and leaving `workspace:` refs alone. Pure over file texts.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::edit;
use crate::model::Kind;
use crate::project::{section, Project, SECTIONS};

/// One requested change to one declaration.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Change {
    /// Set the range of a declaration that exists.
    Set {
        dir: String,
        kind: Kind,
        name: String,
        range: String,
    },
    /// Declare a dependency (or set its range when already declared there).
    Add {
        dir: String,
        kind: Kind,
        name: String,
        range: String,
    },
    Remove {
        dir: String,
        kind: Kind,
        name: String,
    },
    Rename {
        dir: String,
        kind: Kind,
        from: String,
        to: String,
        #[serde(default)]
        range: Option<String>,
    },
}

/// What an edit changed.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Edit {
    pub file: String,
    /// The package's dir; none for a catalog entry.
    pub package: Option<String>,
    pub name: String,
    /// The section's kind, or "catalog".
    pub kind: String,
    pub from: Option<String>,
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renamed_from: Option<String>,
}

/// A declaration a change left alone, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    pub file: String,
    pub package: String,
    pub name: String,
    pub kind: String,
    pub range: String,
    /// "workspace": a `workspace:` ref points at one of the repository's own
    /// packages, so its range is not set and it is not renamed.
    pub reason: String,
}

/// Files the changes may touch: the manifests of their packages and the
/// files that define catalogs.
pub fn files(project: &Project, changes: &[Change]) -> Vec<String> {
    let mut files: Vec<String> = changes
        .iter()
        .map(|change| {
            let dir = match change {
                Change::Set { dir, .. }
                | Change::Add { dir, .. }
                | Change::Remove { dir, .. }
                | Change::Rename { dir, .. } => dir,
            };
            manifest_path(dir)
        })
        .chain(project.catalogs.iter().map(|catalog| catalog.file.clone()))
        .collect();
    files.sort();
    files.dedup();
    files
}

fn manifest_path(dir: &str) -> String {
    if dir == "." || dir.is_empty() {
        "package.json".into()
    } else {
        format!("{dir}/package.json")
    }
}

/// What [`apply`] gives: the files' new texts, what changed, and what it
/// left alone.
pub type Applied = (BTreeMap<String, String>, Vec<Edit>, Vec<Skipped>);

/// The files' texts after the changes, and what changed. `texts` holds the
/// current text of every file [`files`] names that exists.
pub fn apply(
    project: &Project,
    changes: &[Change],
    texts: &BTreeMap<String, String>,
) -> Result<Applied, String> {
    let mut out = texts.clone();
    let mut edits = Vec::new();
    let mut skipped = Vec::new();
    let mut skip = |dir: &str, kind: Kind, name: &str, range: &str| {
        skipped.push(Skipped {
            file: manifest_path(dir),
            package: dir.to_string(),
            name: name.to_string(),
            kind: kind.as_str().to_string(),
            range: range.to_string(),
            reason: "workspace".to_string(),
        })
    };
    let text_of = |out: &BTreeMap<String, String>, file: &str| -> Result<String, String> {
        out.get(file)
            .cloned()
            .ok_or_else(|| format!("cannot read {file}"))
    };
    let declared = |dir: &str, kind: Kind, name: &str| -> Option<String> {
        project
            .packages
            .iter()
            .find(|p| p.dir == dir)?
            .declarations
            .iter()
            .find(|d| d.kind == kind && d.name == name)
            .map(|d| d.range.clone())
    };
    // Catalog entries already set, so a catalog shared by many packages is edited once.
    let mut catalog_done: Vec<(String, String, String)> = Vec::new();

    for change in changes {
        match change {
            Change::Set {
                dir,
                kind,
                name,
                range,
            } => {
                let Some(current) = declared(dir, *kind, name) else {
                    continue;
                };
                if current.starts_with("workspace:") {
                    skip(dir, *kind, name, &current);
                    continue;
                }
                if current.starts_with("catalog:") {
                    let catalog = project.catalog(&current).ok_or_else(|| {
                        format!("{name} in {dir} uses {current}, which no catalog defines")
                    })?;
                    let key = (catalog.file.clone(), catalog.name.clone(), name.clone());
                    if catalog_done.contains(&key) {
                        continue;
                    }
                    catalog_done.push(key);
                    let text = text_of(&out, &catalog.file)?;
                    let (edited, old) = set_catalog(&text, catalog, name, range)?;
                    if old.as_deref() != Some(range.as_str()) {
                        out.insert(catalog.file.clone(), edited);
                        edits.push(Edit {
                            file: catalog.file.clone(),
                            package: None,
                            name: name.clone(),
                            kind: "catalog".into(),
                            from: old,
                            to: Some(range.clone()),
                            renamed_from: None,
                        });
                    }
                    continue;
                }
                // An alias keeps its target: npm:other@^1 becomes npm:other@<range>.
                let range = match current.strip_prefix("npm:") {
                    Some(alias) if !range.starts_with("npm:") => {
                        match alias.get(1..).and_then(|r| r.find('@')) {
                            Some(at) => format!("npm:{}@{range}", &alias[..at + 1]),
                            None => range.clone(),
                        }
                    }
                    _ => range.clone(),
                };
                if range == current {
                    continue;
                }
                let file = manifest_path(dir);
                let text = text_of(&out, &file)?;
                let section = section(*kind).ok_or("no package.json section for that kind")?;
                let (edited, old) = edit::json_set(&text, &[section], name, &range, &[])
                    .map_err(|e| format!("{file}: {e}"))?;
                out.insert(file.clone(), edited);
                edits.push(Edit {
                    file,
                    package: Some(dir.clone()),
                    name: name.clone(),
                    kind: kind.as_str().into(),
                    from: old,
                    to: Some(range),
                    renamed_from: None,
                });
            }
            Change::Add {
                dir,
                kind,
                name,
                range,
            } => {
                if let Some(current) = declared(dir, *kind, name) {
                    if current.starts_with("workspace:") {
                        skip(dir, *kind, name, &current);
                        continue;
                    }
                }
                let file = manifest_path(dir);
                let text = text_of(&out, &file)?;
                let section = section(*kind).ok_or("no package.json section for that kind")?;
                let after: Vec<&str> = SECTIONS.iter().map(|(s, _)| *s).collect();
                let (edited, old) = edit::json_set(&text, &[section], name, range, &after)
                    .map_err(|e| format!("{file}: {e}"))?;
                if old.as_deref() == Some(range.as_str()) {
                    continue;
                }
                out.insert(file.clone(), edited);
                edits.push(Edit {
                    file,
                    package: Some(dir.clone()),
                    name: name.clone(),
                    kind: kind.as_str().into(),
                    from: old,
                    to: Some(range.clone()),
                    renamed_from: None,
                });
            }
            Change::Remove { dir, kind, name } => {
                let file = manifest_path(dir);
                let text = text_of(&out, &file)?;
                let section = section(*kind).ok_or("no package.json section for that kind")?;
                let (edited, old) = edit::json_remove(&text, &[section], name)
                    .map_err(|e| format!("{file}: {e}"))?;
                if old.is_none() {
                    continue;
                }
                out.insert(file.clone(), edited);
                edits.push(Edit {
                    file,
                    package: Some(dir.clone()),
                    name: name.clone(),
                    kind: kind.as_str().into(),
                    from: old,
                    to: None,
                    renamed_from: None,
                });
            }
            Change::Rename {
                dir,
                kind,
                from,
                to,
                range,
            } => {
                let Some(current) = declared(dir, *kind, from) else {
                    continue;
                };
                if current.starts_with("workspace:") {
                    skip(dir, *kind, from, &current);
                    continue;
                }
                // A catalog ref keeps pointing at the catalog: the new name gets an entry there.
                if range.is_none() && current.starts_with("catalog:") {
                    if let Some(catalog) = project.catalog(&current) {
                        if let Some((_, value)) = catalog.entries.iter().find(|(n, _)| n == from) {
                            let key = (catalog.file.clone(), catalog.name.clone(), to.clone());
                            if !catalog_done.contains(&key) {
                                catalog_done.push(key);
                                let text = text_of(&out, &catalog.file)?;
                                let (edited, old) = set_catalog(&text, catalog, to, value)?;
                                if old.is_none() {
                                    out.insert(catalog.file.clone(), edited);
                                    edits.push(Edit {
                                        file: catalog.file.clone(),
                                        package: None,
                                        name: to.clone(),
                                        kind: "catalog".into(),
                                        from: None,
                                        to: Some(value.clone()),
                                        renamed_from: None,
                                    });
                                }
                            }
                            // The old entry goes when no declaration left uses it.
                            let still_used = project.packages.iter().any(|p| {
                                p.declarations.iter().any(|d| {
                                    d.name == *from
                                        && project.catalog(&d.range).is_some_and(|c| {
                                            c.name == catalog.name && c.file == catalog.file
                                        })
                                        && !changes.iter().any(|c| matches!(c, Change::Rename { dir, kind, from: f, .. } if *dir == p.dir && *kind == d.kind && f == from))
                                })
                            });
                            let gone = (
                                catalog.file.clone(),
                                catalog.name.clone(),
                                format!("-{from}"),
                            );
                            if !still_used && !catalog_done.contains(&gone) {
                                catalog_done.push(gone);
                                let text = text_of(&out, &catalog.file)?;
                                let (edited, old) = remove_catalog(&text, catalog, from)?;
                                if old.is_some() {
                                    out.insert(catalog.file.clone(), edited);
                                    edits.push(Edit {
                                        file: catalog.file.clone(),
                                        package: None,
                                        name: from.clone(),
                                        kind: "catalog".into(),
                                        from: old,
                                        to: None,
                                        renamed_from: None,
                                    });
                                }
                            }
                        }
                    }
                }
                let file = manifest_path(dir);
                let text = text_of(&out, &file)?;
                let section = section(*kind).ok_or("no package.json section for that kind")?;
                let (edited, old) =
                    edit::json_rename(&text, &[section], from, to, range.as_deref())
                        .map_err(|e| format!("{file}: {e}"))?;
                if old.is_none() {
                    continue;
                }
                out.insert(file.clone(), edited);
                edits.push(Edit {
                    file,
                    package: Some(dir.clone()),
                    name: to.clone(),
                    kind: kind.as_str().into(),
                    from: old.clone(),
                    to: Some(range.clone().unwrap_or_else(|| old.unwrap_or_default())),
                    renamed_from: Some(from.clone()),
                });
            }
        }
    }
    out.retain(|file, text| texts.get(file) != Some(text));
    Ok((out, edits, skipped))
}

fn set_catalog(
    text: &str,
    catalog: &crate::project::Catalog,
    name: &str,
    range: &str,
) -> Result<(String, Option<String>), String> {
    if catalog.file.ends_with(".json") {
        let path: Vec<&str> = catalog.path.iter().map(String::as_str).collect();
        edit::json_set(text, &path, name, range, &[])
    } else {
        edit::yaml_catalog_set(text, &catalog.name, name, range)
    }
    .map_err(|e| format!("{}: {e}", catalog.file))
}

fn remove_catalog(
    text: &str,
    catalog: &crate::project::Catalog,
    name: &str,
) -> Result<(String, Option<String>), String> {
    if catalog.file.ends_with(".json") {
        let path: Vec<&str> = catalog.path.iter().map(String::as_str).collect();
        edit::json_remove(text, &path, name)
    } else {
        edit::yaml_catalog_remove(text, &catalog.name, name)
    }
    .map_err(|e| format!("{}: {e}", catalog.file))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Project, BTreeMap<String, String>) {
        let files: BTreeMap<String, String> = [
            ("package.json", "{\n  \"name\": \"root\",\n  \"private\": true\n}\n"),
            ("pnpm-workspace.yaml", "packages:\n  - packages/*\ncatalog:\n  react: ^18.2.0\n"),
            ("packages/web/package.json", "{\n  \"name\": \"web\",\n  \"dependencies\": {\n    \"react\": \"catalog:\",\n    \"shared\": \"workspace:*\",\n    \"sw\": \"npm:string-width@^4.2.0\"\n  }\n}\n"),
            ("packages/api/package.json", "{\n  \"name\": \"api\",\n  \"dependencies\": {\n    \"react\": \"catalog:\"\n  }\n}\n"),
            ("packages/shared/package.json", "{\n  \"name\": \"shared\",\n  \"version\": \"1.0.0\"\n}\n"),
        ]
        .iter()
        .map(|(p, t)| (p.to_string(), t.to_string()))
        .collect();
        let paths: Vec<String> = files
            .keys()
            .filter(|p| p.ends_with("package.json"))
            .cloned()
            .collect();
        let project = Project::read(&paths, &["pnpm-lock.yaml"], &|p| files.get(p).cloned());
        (project, files)
    }

    fn set(dir: &str, name: &str, range: &str) -> Change {
        Change::Set {
            dir: dir.into(),
            kind: Kind::Prod,
            name: name.into(),
            range: range.into(),
        }
    }

    #[test]
    fn a_catalog_ref_is_bumped_once_in_its_catalog() {
        let (project, files) = fixture();
        let changes = [
            set("packages/web", "react", "^19.0.0"),
            set("packages/api", "react", "^19.0.0"),
        ];
        let (out, edits, _) = apply(&project, &changes, &files).unwrap();
        assert_eq!(out.keys().collect::<Vec<_>>(), ["pnpm-workspace.yaml"]);
        assert!(out["pnpm-workspace.yaml"].contains("  react: ^19.0.0\n"));
        assert_eq!(edits.len(), 1);
        assert_eq!(
            (edits[0].kind.as_str(), edits[0].from.as_deref()),
            ("catalog", Some("^18.2.0"))
        );
    }

    #[test]
    fn workspace_refs_are_left_alone_and_aliases_keep_their_target() {
        let (project, files) = fixture();
        let (out, edits, skipped) = apply(
            &project,
            &[
                set("packages/web", "shared", "^2.0.0"),
                set("packages/web", "sw", "^5.0.0"),
            ],
            &files,
        )
        .unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(
            skipped,
            [Skipped {
                file: "packages/web/package.json".into(),
                package: "packages/web".into(),
                name: "shared".into(),
                kind: "prod".into(),
                range: "workspace:*".into(),
                reason: "workspace".into(),
            }]
        );
        assert!(out["packages/web/package.json"].contains("\"sw\": \"npm:string-width@^5.0.0\""));
        assert!(out["packages/web/package.json"].contains("\"shared\": \"workspace:*\""));
    }

    #[test]
    fn renaming_a_catalog_ref_moves_the_catalog_entry() {
        let (project, files) = fixture();
        let rename = |dir: &str| Change::Rename {
            dir: dir.into(),
            kind: Kind::Prod,
            from: "react".into(),
            to: "preact".into(),
            range: None,
        };
        let (out, _, _) = apply(&project, &[rename("packages/web")], &files).unwrap();
        // api still uses react from the catalog.
        assert!(
            out["pnpm-workspace.yaml"].contains("  react: ^18.2.0\n  preact: ^18.2.0\n"),
            "{}",
            out["pnpm-workspace.yaml"]
        );
        let (out, edits, _) = apply(
            &project,
            &[rename("packages/web"), rename("packages/api")],
            &files,
        )
        .unwrap();
        assert_eq!(
            out["pnpm-workspace.yaml"],
            "packages:\n  - packages/*\ncatalog:\n  preact: ^18.2.0\n"
        );
        assert!(out["packages/api/package.json"].contains("\"preact\": \"catalog:\""));
        assert_eq!(edits.iter().filter(|e| e.kind == "catalog").count(), 2);
    }

    #[test]
    fn adding_creates_the_section() {
        let (project, files) = fixture();
        let (out, edits, _) = apply(
            &project,
            &[Change::Add {
                dir: ".".into(),
                kind: Kind::Dev,
                name: "vitest".into(),
                range: "^3.0.0".into(),
            }],
            &files,
        )
        .unwrap();
        assert_eq!(out["package.json"], "{\n  \"name\": \"root\",\n  \"private\": true,\n  \"devDependencies\": {\n    \"vitest\": \"^3.0.0\"\n  }\n}\n");
        assert_eq!(edits[0].from, None);
    }
}
