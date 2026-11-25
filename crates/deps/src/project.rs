//! The repository's own packages, read from their manifests: no lockfile
//! needed. npm-family package.json files, the workspaces the root declares,
//! catalogs (pnpm, bun, yarn) and the package manager.
//!
//! [`Project::read`] is pure over a list of paths and a read function, so it
//! works on a working tree, a git tree or an in-memory one alike;
//! [`discover`] lists the package.json files of a directory on disk.

use std::path::Path;

use serde::Serialize;

use crate::edit;
use crate::lockfiles::path_glob_match;
use crate::model::Kind;
use crate::semver;

/// Sections of a package.json, in the order they are read, with their kinds.
pub const SECTIONS: [(&str, Kind); 4] = [
    ("dependencies", Kind::Prod),
    ("devDependencies", Kind::Dev),
    ("peerDependencies", Kind::Peer),
    ("optionalDependencies", Kind::Optional),
];

/// The package.json section of a kind.
pub fn section(kind: Kind) -> Option<&'static str> {
    SECTIONS
        .iter()
        .find(|(_, k)| *k == kind)
        .map(|(section, _)| *section)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Declared {
    pub name: String,
    /// As written.
    pub range: String,
    pub kind: Kind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// Relative to the root, "." for the root.
    pub dir: String,
    /// The package.json's path, relative to the root.
    pub manifest: String,
    pub name: Option<String>,
    pub version: Option<String>,
    pub declarations: Vec<Declared>,
    /// One of the repository's packages: the root, or a workspace the root
    /// declares. Other package.json files (fixtures, examples, a repository
    /// without workspaces) are read too, but are not members.
    pub member: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    /// "default" for `catalog:`.
    pub name: String,
    /// The file that defines it.
    pub file: String,
    /// Where the catalog sits in that file: JSON keys for package.json.
    #[serde(skip)]
    pub path: Vec<String>,
    pub entries: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageManagerName {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

impl PackageManagerName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
            Self::Bun => "bun",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageManager {
    pub name: PackageManagerName,
    /// From `packageManager` in the root package.json.
    pub version: Option<String>,
    /// The root lockfile it keeps, when there is one.
    pub lockfile: Option<String>,
    pub detected_from: &'static str,
}

impl PackageManager {
    /// The command that refreshes the lockfile, or installs.
    pub fn install_command(&self, lockfile_only: bool) -> String {
        let classic_yarn = self.name == PackageManagerName::Yarn
            && self.version.as_deref().is_some_and(|v| v.starts_with("1."));
        match (self.name, lockfile_only) {
            (PackageManagerName::Npm, true) => {
                "npm install --package-lock-only --no-audit --no-fund"
            }
            (PackageManagerName::Npm, false) => "npm install --no-audit --no-fund",
            (PackageManagerName::Pnpm, true) => "pnpm install --lockfile-only",
            (PackageManagerName::Pnpm, false) => "pnpm install",
            (PackageManagerName::Yarn, _) if classic_yarn => "yarn install --non-interactive",
            (PackageManagerName::Yarn, true) => "yarn install --mode=update-lockfile",
            (PackageManagerName::Yarn, false) => "yarn install",
            (PackageManagerName::Bun, true) => "bun install --lockfile-only",
            (PackageManagerName::Bun, false) => "bun install",
        }
        .to_string()
    }
}

/// Root lockfiles by package manager, in detection order.
const LOCKFILES: [(&str, PackageManagerName); 6] = [
    ("pnpm-lock.yaml", PackageManagerName::Pnpm),
    ("bun.lock", PackageManagerName::Bun),
    ("bun.lockb", PackageManagerName::Bun),
    ("yarn.lock", PackageManagerName::Yarn),
    ("package-lock.json", PackageManagerName::Npm),
    ("npm-shrinkwrap.json", PackageManagerName::Npm),
];

/// A file name the reader may ask for, besides package.json files.
pub const CONTEXT_FILES: [&str; 3] = ["pnpm-workspace.yaml", ".yarnrc.yml", "package.json"];

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    /// The root first, then the rest by dir.
    pub packages: Vec<Manifest>,
    pub catalogs: Vec<Catalog>,
    pub package_manager: Option<PackageManager>,
    /// Manifests that could not be read, with why.
    pub unreadable: Vec<(String, String)>,
}

/// What a declaration's range means.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    pub protocol: &'static str,
    pub resolved: Option<String>,
    /// For a catalog ref: its catalog, as (name, file).
    pub catalog: Option<(String, String)>,
    /// For a workspace ref: the local package's index.
    pub local: Option<usize>,
    pub readable: bool,
}

impl Project {
    /// Read the project at the root of `paths`: every package.json path
    /// (relative, with `/`) under it. `read` gives a file's text by relative
    /// path. The members are the root package.json and the workspaces its
    /// `workspaces`, or pnpm-workspace.yaml's `packages`, declare; without
    /// either, only the root. Every other path is read too, as a non-member.
    /// `present` are the root lockfiles that exist, for the package manager.
    pub fn read(paths: &[String], present: &[&str], read: &dyn Fn(&str) -> Option<String>) -> Self {
        let mut project = Project::default();
        let root_text = read("package.json");
        let root_json: Option<serde_json::Value> = root_text
            .as_deref()
            .and_then(|text| serde_json::from_str(text).ok());
        let pnpm_workspace = read("pnpm-workspace.yaml");

        let mut globs: Option<Vec<String>> = pnpm_workspace
            .as_deref()
            .map(|text| edit::yaml_list(text, "packages"))
            .filter(|globs| !globs.is_empty());
        if globs.is_none() {
            globs = root_json.as_ref().and_then(|json| {
                let workspaces = json.get("workspaces")?;
                let list = workspaces
                    .get("packages")
                    .unwrap_or(workspaces)
                    .as_array()?;
                Some(
                    list.iter()
                        .filter_map(|glob| glob.as_str().map(String::from))
                        .collect(),
                )
            });
        }
        let normalize = |glob: &str| {
            glob.trim_start_matches("./")
                .trim_end_matches('/')
                .trim_end_matches("/package.json")
                .to_string()
        };
        let mut dirs: Vec<String> = paths
            .iter()
            .filter_map(|path| {
                Some(match path.as_str() {
                    "package.json" => ".".to_string(),
                    other => other.strip_suffix("/package.json")?.to_string(),
                })
            })
            .collect();
        let member = |dir: &str| {
            if dir == "." {
                return true;
            }
            let Some(globs) = &globs else {
                return false;
            };
            let included = globs
                .iter()
                .filter(|glob| !glob.starts_with('!'))
                .any(|glob| path_glob_match(&normalize(glob), dir));
            let excluded = globs
                .iter()
                .filter_map(|glob| glob.strip_prefix('!'))
                .any(|glob| path_glob_match(&normalize(glob), dir));
            included && !excluded
        };
        dirs.sort_by(|a, b| (a != ".").cmp(&(b != ".")).then(a.cmp(b)));
        dirs.dedup();

        for dir in dirs {
            let manifest = if dir == "." {
                "package.json".to_string()
            } else {
                format!("{dir}/package.json")
            };
            let Some(text) = read(&manifest) else {
                continue;
            };
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(json) => project.packages.push(Manifest {
                    name: json.get("name").and_then(|v| v.as_str()).map(String::from),
                    version: json
                        .get("version")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    declarations: declarations(&json),
                    member: member(&dir),
                    dir,
                    manifest,
                }),
                Err(error) => project.unreadable.push((manifest, error.to_string())),
            }
        }

        // Catalogs: pnpm-workspace.yaml, then bun's package.json, then yarn's .yarnrc.yml.
        if let Some(text) = &pnpm_workspace {
            for (name, entries) in edit::yaml_catalogs(text) {
                project.catalogs.push(Catalog {
                    name,
                    file: "pnpm-workspace.yaml".into(),
                    path: Vec::new(),
                    entries,
                });
            }
        }
        if let Some(text) = root_text.as_deref() {
            for base in [vec![], vec!["workspaces"]] {
                let mut path = base.clone();
                path.push("catalog");
                if let Ok(entries) = edit::json_entries(text, &path) {
                    if !entries.is_empty() {
                        project.catalogs.push(Catalog {
                            name: "default".into(),
                            file: "package.json".into(),
                            path: path.iter().map(|s| s.to_string()).collect(),
                            entries,
                        });
                    }
                }
                let mut path = base.clone();
                path.push("catalogs");
                let names: Vec<String> = root_json
                    .as_ref()
                    .and_then(|json| path.iter().try_fold(json, |json, key| json.get(*key)))
                    .and_then(|v| v.as_object())
                    .map(|o| o.keys().cloned().collect())
                    .unwrap_or_default();
                for name in names {
                    let mut path = path.clone();
                    path.push(&name);
                    let entries = edit::json_entries(text, &path).unwrap_or_default();
                    project.catalogs.push(Catalog {
                        name: name.clone(),
                        file: "package.json".into(),
                        path: path.iter().map(|s| s.to_string()).collect(),
                        entries,
                    });
                }
            }
        }
        if let Some(text) = read(".yarnrc.yml") {
            for (name, entries) in edit::yaml_catalogs(&text) {
                project.catalogs.push(Catalog {
                    name,
                    file: ".yarnrc.yml".into(),
                    path: Vec::new(),
                    entries,
                });
            }
        }

        project.package_manager =
            detect_package_manager(root_json.as_ref(), present, pnpm_workspace.is_some());
        project
    }

    /// The catalog a `catalog:` ref names: "catalog:" and "catalog:default"
    /// name the default one.
    pub fn catalog(&self, reference: &str) -> Option<&Catalog> {
        let name = reference.strip_prefix("catalog:")?.trim();
        let name = if name.is_empty() { "default" } else { name };
        self.catalogs.iter().find(|catalog| catalog.name == name)
    }

    /// What `range` means for a declaration of `name`.
    pub fn resolve(&self, name: &str, range: &str) -> Resolved {
        let readable = |range: &str| semver::valid_range(crate::Ecosystem::Npm, range);
        if range.starts_with("catalog:") {
            let catalog = self.catalog(range);
            let resolved = catalog.and_then(|catalog| {
                catalog
                    .entries
                    .iter()
                    .find(|(entry, _)| entry == name)
                    .map(|(_, range)| range.clone())
            });
            let readable = resolved.as_deref().is_some_and(readable);
            return Resolved {
                protocol: "catalog",
                catalog: catalog.map(|c| (c.name.clone(), c.file.clone())),
                resolved,
                local: None,
                readable,
            };
        }
        if let Some(spec) = range.strip_prefix("workspace:") {
            let local = self
                .packages
                .iter()
                .position(|package| package.member && package.name.as_deref() == Some(name))
                .or_else(|| {
                    // workspace:packages/x names a path.
                    let dir = spec.trim_start_matches("./").trim_end_matches('/');
                    self.packages.iter().position(|package| package.dir == dir)
                });
            let version = local.and_then(|i| self.packages[i].version.clone());
            let resolved = match spec {
                "*" => version,
                "^" | "~" => version.map(|v| format!("{spec}{v}")),
                spec if readable(spec) => Some(spec.to_string()),
                _ => version,
            };
            let readable = resolved.as_deref().is_some_and(readable);
            return Resolved {
                protocol: "workspace",
                resolved,
                catalog: None,
                local,
                readable,
            };
        }
        if let Some(alias) = range.strip_prefix("npm:") {
            let resolved = alias
                .get(1..)
                .and_then(|rest| rest.find('@'))
                .map(|at| alias[at + 2..].to_string());
            let readable = resolved.as_deref().is_some_and(readable);
            return Resolved {
                protocol: "alias",
                resolved,
                catalog: None,
                local: None,
                readable,
            };
        }
        let registry = readable(range);
        Resolved {
            protocol: if registry { "registry" } else { "other" },
            resolved: Some(range.to_string()),
            catalog: None,
            local: None,
            readable: registry,
        }
    }
}

/// A package.json's declarations, in section order.
pub fn declarations(json: &serde_json::Value) -> Vec<Declared> {
    let mut out = Vec::new();
    for (section, kind) in SECTIONS {
        for (name, range) in json
            .get(section)
            .and_then(|v| v.as_object())
            .into_iter()
            .flatten()
        {
            out.push(Declared {
                name: name.clone(),
                range: range.as_str().unwrap_or_default().to_string(),
                kind,
            });
        }
    }
    out
}

fn detect_package_manager(
    root: Option<&serde_json::Value>,
    present: &[&str],
    pnpm_workspace: bool,
) -> Option<PackageManager> {
    let lockfile_of = |name: PackageManagerName| {
        LOCKFILES
            .iter()
            .find(|(file, pm)| *pm == name && present.contains(file))
            .map(|(file, _)| file.to_string())
    };
    if let Some(field) = root
        .and_then(|json| json.get("packageManager"))
        .and_then(|v| v.as_str())
    {
        let (name, version) = field.split_once('@').unwrap_or((field, ""));
        let name = match name {
            "npm" => Some(PackageManagerName::Npm),
            "pnpm" => Some(PackageManagerName::Pnpm),
            "yarn" => Some(PackageManagerName::Yarn),
            "bun" => Some(PackageManagerName::Bun),
            _ => None,
        };
        if let Some(name) = name {
            // "pnpm@9.1.0+sha512.abc" keeps the version only.
            let version = version.split('+').next().unwrap_or_default();
            return Some(PackageManager {
                name,
                version: (!version.is_empty()).then(|| version.to_string()),
                lockfile: lockfile_of(name),
                detected_from: "packageManager",
            });
        }
    }
    if let Some((file, name)) = LOCKFILES.iter().find(|(file, _)| present.contains(file)) {
        return Some(PackageManager {
            name: *name,
            version: None,
            lockfile: Some(file.to_string()),
            detected_from: "lockfile",
        });
    }
    if pnpm_workspace {
        return Some(PackageManager {
            name: PackageManagerName::Pnpm,
            version: None,
            lockfile: None,
            detected_from: "pnpm-workspace.yaml",
        });
    }
    root.map(|_| PackageManager {
        name: PackageManagerName::Npm,
        version: None,
        lockfile: None,
        detected_from: "default",
    })
}

/// The registry hosts a package manager installs from, with where each one
/// comes from: the default (registry.npmjs.org; registry.yarnpkg.com for
/// Yarn), replaced by `registry=` in .npmrc, plus scoped `@scope:registry=`
/// ones; for Yarn 2+, `npmRegistryServer` in .yarnrc.yml instead. `read`
/// gives a file's text relative to the package manager's directory.
pub fn registries(
    manager: &PackageManager,
    read: &dyn Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let host = |url: &str| -> Option<String> {
        let url = url.trim().trim_matches(|c| c == '"' || c == '\'');
        let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
        let host = rest.split(['/', ':']).next()?.trim();
        (!host.is_empty()).then(|| host.to_ascii_lowercase())
    };
    let classic_yarn = manager
        .version
        .as_deref()
        .is_some_and(|v| v.starts_with("1."));
    let berry = manager.name == PackageManagerName::Yarn
        && !classic_yarn
        && (manager.version.is_some() || read(".yarnrc.yml").is_some());
    let mut default = if manager.name == PackageManagerName::Yarn {
        (
            "registry.yarnpkg.com".to_string(),
            "Yarn's default".to_string(),
        )
    } else {
        (
            "registry.npmjs.org".to_string(),
            format!("{}'s default", manager.name.as_str()),
        )
    };
    let mut scoped: Vec<(String, String)> = Vec::new();
    if berry {
        for line in read(".yarnrc.yml").unwrap_or_default().lines() {
            let indented = line.starts_with([' ', '\t']);
            let Some(value) = line.trim().strip_prefix("npmRegistryServer:") else {
                continue;
            };
            if let Some(h) = host(value) {
                // Top level: the registry; indented: a scope's (npmScopes).
                if indented {
                    scoped.push((h, ".yarnrc.yml".into()));
                } else {
                    default = (h, ".yarnrc.yml".into());
                }
            }
        }
    } else {
        for line in read(".npmrc").unwrap_or_default().lines() {
            let line = line.trim();
            if line.starts_with(['#', ';']) {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            if key == "registry" {
                if let Some(h) = host(value) {
                    default = (h, ".npmrc".into());
                }
            } else if key.ends_with(":registry") {
                if let Some(h) = host(value) {
                    scoped.push((h, ".npmrc".into()));
                }
            }
        }
    }
    let mut found = vec![default];
    for entry in scoped {
        if !found.iter().any(|(h, _)| *h == entry.0) {
            found.push(entry);
        }
    }
    found
}

/// Every package.json under `root`, relative with `/`, root first:
/// `.gitignore` is respected and `node_modules` skipped.
pub fn discover(root: &Path) -> Vec<String> {
    let mut found: Vec<String> = ignore::WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|entry| {
            !matches!(
                entry.file_name().to_str(),
                Some("node_modules" | ".git" | "target")
            )
        })
        .build()
        .flatten()
        .filter(|entry| entry.file_name() == "package.json")
        .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|entry| {
            let relative = entry.path().strip_prefix(root).ok()?;
            Some(relative.to_string_lossy().replace('\\', "/"))
        })
        .collect();
    found.sort_by(|a, b| {
        (a != "package.json")
            .cmp(&(b != "package.json"))
            .then(a.cmp(b))
    });
    found
}

/// The root files the package manager is detected from that exist.
pub fn present_root_files(root: &Path) -> Vec<&'static str> {
    LOCKFILES
        .iter()
        .map(|(file, _)| *file)
        .filter(|file| root.join(file).is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn project(files: &[(&str, &str)], present: &[&str]) -> Project {
        let files: HashMap<String, String> = files
            .iter()
            .map(|(path, text)| (path.to_string(), text.to_string()))
            .collect();
        let paths: Vec<String> = files
            .keys()
            .filter(|path| path.ends_with("package.json"))
            .cloned()
            .collect();
        Project::read(&paths, present, &|path| files.get(path).cloned())
    }

    #[test]
    fn workspaces_pick_the_packages_and_the_root_comes_first() {
        let p = project(
            &[
                (
                    "package.json",
                    r#"{ "name": "root", "workspaces": ["packages/*", "!packages/old"], "devDependencies": { "typescript": "^5.0.0" } }"#,
                ),
                (
                    "packages/web/package.json",
                    r#"{ "name": "web", "version": "1.0.0", "dependencies": { "react": "^18.2.0" } }"#,
                ),
                ("packages/old/package.json", r#"{ "name": "old" }"#),
                ("examples/x/package.json", r#"{ "name": "x" }"#),
            ],
            &["package-lock.json"],
        );
        let dirs: Vec<&str> = p
            .packages
            .iter()
            .filter(|p| p.member)
            .map(|p| p.dir.as_str())
            .collect();
        assert_eq!(dirs, [".", "packages/web"]);
        let web = p.packages.iter().find(|p| p.dir == "packages/web").unwrap();
        assert_eq!(web.declarations[0].name, "react");
        let pm = p.package_manager.unwrap();
        assert_eq!(
            (pm.name, pm.lockfile.as_deref(), pm.detected_from),
            (
                PackageManagerName::Npm,
                Some("package-lock.json"),
                "lockfile"
            )
        );
    }

    #[test]
    fn without_workspaces_only_the_root_is_a_member() {
        let p = project(
            &[
                ("package.json", r#"{ "name": "root" }"#),
                ("examples/x/package.json", r#"{ "name": "x" }"#),
            ],
            &[],
        );
        let members: Vec<(&str, bool)> = p
            .packages
            .iter()
            .map(|p| (p.dir.as_str(), p.member))
            .collect();
        assert_eq!(members, [(".", true), ("examples/x", false)]);
        assert_eq!(p.package_manager.unwrap().detected_from, "default");
    }

    #[test]
    fn registries_come_from_npmrc_and_yarnrc() {
        let pm = |name, version: Option<&str>| PackageManager {
            name,
            version: version.map(String::from),
            lockfile: None,
            detected_from: "lockfile",
        };
        let hosts = |manager: PackageManager, files: &[(&str, &str)]| -> Vec<String> {
            registries(&manager, &|path| {
                files
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, t)| t.to_string())
            })
            .into_iter()
            .map(|(host, from)| format!("{host} ({from})"))
            .collect()
        };
        assert_eq!(
            hosts(pm(PackageManagerName::Npm, None), &[]),
            ["registry.npmjs.org (npm's default)"]
        );
        assert_eq!(
            hosts(
                pm(PackageManagerName::Pnpm, None),
                &[(".npmrc", "# x\nregistry=https://npm.acme.dev/repo/\n@acme:registry=https://npm.pkg.github.com\n")]
            ),
            ["npm.acme.dev (.npmrc)", "npm.pkg.github.com (.npmrc)"]
        );
        assert_eq!(
            hosts(pm(PackageManagerName::Yarn, Some("4.5.0")), &[]),
            ["registry.yarnpkg.com (Yarn's default)"]
        );
        assert_eq!(
            hosts(
                pm(PackageManagerName::Yarn, Some("4.5.0")),
                &[(".yarnrc.yml", "npmRegistryServer: \"https://registry.npmjs.org\"\nnpmScopes:\n  acme:\n    npmRegistryServer: https://npm.acme.dev\n")]
            ),
            ["registry.npmjs.org (.yarnrc.yml)", "npm.acme.dev (.yarnrc.yml)"]
        );
        assert_eq!(
            hosts(pm(PackageManagerName::Yarn, Some("1.22.19")), &[]),
            ["registry.yarnpkg.com (Yarn's default)"]
        );
    }

    #[test]
    fn pnpm_catalogs_and_workspace_refs_resolve() {
        let p = project(
            &[
                ("package.json", r#"{ "name": "root", "packageManager": "pnpm@9.12.0+sha512.abc" }"#),
                ("pnpm-workspace.yaml", "packages:\n  - packages/*\ncatalog:\n  react: ^18.2.0\ncatalogs:\n  legacy:\n    react: ^17.0.2\n"),
                ("packages/web/package.json", r#"{ "name": "web", "dependencies": { "react": "catalog:", "shared": "workspace:^" } }"#),
                ("packages/shared/package.json", r#"{ "name": "shared", "version": "1.2.0", "dependencies": { "react": "catalog:legacy" } }"#),
            ],
            &["pnpm-lock.yaml"],
        );
        let pm = p.package_manager.as_ref().unwrap();
        assert_eq!(
            (pm.name, pm.version.as_deref()),
            (PackageManagerName::Pnpm, Some("9.12.0"))
        );
        let r = p.resolve("react", "catalog:");
        assert_eq!(
            (r.protocol, r.resolved.as_deref(), r.readable),
            ("catalog", Some("^18.2.0"), true)
        );
        assert_eq!(
            r.catalog,
            Some(("default".into(), "pnpm-workspace.yaml".into()))
        );
        assert_eq!(
            p.resolve("react", "catalog:legacy").resolved.as_deref(),
            Some("^17.0.2")
        );
        let w = p.resolve("shared", "workspace:^");
        assert_eq!(
            (w.protocol, w.resolved.as_deref()),
            ("workspace", Some("^1.2.0"))
        );
        assert_eq!(p.packages[w.local.unwrap()].dir, "packages/shared");
        assert_eq!(
            p.resolve("x", "npm:other@^2.0.0").resolved.as_deref(),
            Some("^2.0.0")
        );
        let git = p.resolve("x", "github:a/b");
        assert_eq!((git.protocol, git.readable), ("other", false));
        assert_eq!(p.resolve("x", "latest").protocol, "other");
    }

    #[test]
    fn bun_catalogs_live_in_package_json() {
        let p = project(
            &[(
                "package.json",
                r#"{ "name": "root", "workspaces": { "packages": ["packages/*"], "catalog": { "react": "^18.2.0" }, "catalogs": { "testing": { "vitest": "^3.0.0" } } } }"#,
            )],
            &["bun.lock"],
        );
        assert_eq!(
            p.resolve("react", "catalog:").resolved.as_deref(),
            Some("^18.2.0")
        );
        assert_eq!(
            p.resolve("vitest", "catalog:testing").resolved.as_deref(),
            Some("^3.0.0")
        );
        assert_eq!(p.package_manager.unwrap().name, PackageManagerName::Bun);
    }

    #[test]
    fn install_commands() {
        let pm = |name, version: Option<&str>| PackageManager {
            name,
            version: version.map(String::from),
            lockfile: None,
            detected_from: "lockfile",
        };
        assert_eq!(
            pm(PackageManagerName::Pnpm, None).install_command(true),
            "pnpm install --lockfile-only"
        );
        assert_eq!(
            pm(PackageManagerName::Yarn, Some("1.22.19")).install_command(true),
            "yarn install --non-interactive"
        );
        assert_eq!(
            pm(PackageManagerName::Yarn, Some("4.5.0")).install_command(true),
            "yarn install --mode=update-lockfile"
        );
    }
}
