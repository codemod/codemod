use std::collections::HashSet;

use codemod_deps::lockfiles::{self, CargoManifests};
use codemod_deps::registry::describe;
use codemod_deps::{Ecosystem, Graph, Kind, Release, Walk};
use serde_json::json;

const YARN_LOCK: &str = r#"
__metadata:
  version: 8

"app@workspace:.":
  version: 0.0.0-use.local
  resolution: "app@workspace:."
  languageName: unknown
  linkType: soft

"table@npm:^3.0.0":
  version: 3.2.5
  resolution: "table@npm:3.2.5"
  dependencies:
    dnd: "npm:^13.0.0"
  peerDependencies:
    react: ^16.8.0 || ^17 || ^18
  languageName: node
  linkType: hard

"dnd@npm:^13.0.0":
  version: 13.0.0
  resolution: "dnd@npm:13.0.0"
  peerDependencies:
    react: ^16.8.5 || ^17.0.0 || ^18.0.0
  languageName: node
  linkType: hard
"#;

const APP_PACKAGE_JSON: &str = r#"{ "name": "app", "dependencies": { "table": "^3.0.0" }, "devDependencies": { "dnd": "^13.0.0" }, "peerDependencies": { "react": "^18.0.0" } }"#;

fn yarn_app() -> Graph {
    lockfiles::yarn(YARN_LOCK, &|path| {
        (path == "package.json").then(|| APP_PACKAGE_JSON.to_string())
    })
    .unwrap()
}

/// `from kind to range` for each edge.
fn described(walk: &Walk, edges: Vec<usize>) -> Vec<String> {
    edges
        .into_iter()
        .map(|index| {
            let edge = &walk.graph.edges[index];
            let to = edge
                .to
                .map_or_else(|| edge.name.clone(), |to| walk.graph.label(to));
            let range = edge.range.as_deref().unwrap_or("?");
            format!(
                "{} {} {to} {range}",
                walk.graph.label(edge.from),
                edge.kind.as_str()
            )
        })
        .collect()
}

fn workspace_dependencies(graph: Graph, kinds: &[Kind]) -> Vec<String> {
    let walk = Walk::new(graph);
    let workspace = walk.workspaces()[0];
    described(&walk, walk.dependencies(workspace, kinds))
}

fn chains_to(graph: Graph, name: &str, kinds: &[Kind]) -> Vec<String> {
    let walk = Walk::new(graph);
    let targets: HashSet<usize> = walk.nodes_named(name).iter().copied().collect();
    walk.chains(&walk.workspaces(), &targets, kinds)
        .into_iter()
        .map(|chain| {
            chain
                .iter()
                .map(|&id| walk.graph.label(id))
                .collect::<Vec<_>>()
                .join(" > ")
        })
        .collect()
}

#[test]
fn a_yarn_workspace_dependency_kind_comes_from_its_package_json() {
    assert_eq!(
        workspace_dependencies(yarn_app(), &[Kind::Dev]),
        ["app@0.0.0-use.local dev dnd@13.0.0 ^13.0.0"]
    );
}

#[test]
fn a_chain_is_the_shortest_path_from_each_workspace_over_the_kinds_asked_for() {
    assert_eq!(
        chains_to(yarn_app(), "dnd", &[Kind::Prod]),
        ["app@0.0.0-use.local > table@3.2.5 > dnd@13.0.0"]
    );
}

#[test]
fn edges_declaring_a_package_include_unresolved_peer_ranges() {
    let walk = Walk::new(yarn_app());
    assert_eq!(
        described(&walk, walk.declaring("react", &[Kind::Peer])),
        [
            "app@0.0.0-use.local peer react ^18.0.0",
            "table@3.2.5 peer react ^16.8.0 || ^17 || ^18",
            "dnd@13.0.0 peer react ^16.8.5 || ^17.0.0 || ^18.0.0"
        ]
    );
}

#[test]
fn a_yarn_v1_lockfile_is_reported_as_not_read() {
    let error = lockfiles::yarn(
        "# yarn lockfile v1\n\na@^1:\n  version \"1.0.0\"\n",
        &|_| None,
    )
    .unwrap_err();
    assert_eq!(error, "Yarn v1 lockfiles are not read yet");
}

#[test]
fn pnpm_snapshots_with_peer_suffixes_are_one_package_version_with_its_declared_range() {
    let lock = r#"
lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      table:
        specifier: ^3.0.0
        version: 3.2.5(react@18.3.1)
packages:
  table@3.2.5:
    peerDependencies:
      react: ^18
  react@18.3.1: {}
snapshots:
  table@3.2.5(react@18.3.1):
    dependencies:
      react: 18.3.1
  react@18.3.1: {}
"#;
    let graph = lockfiles::pnpm(lock, &|_| Some(r#"{ "name": "app" }"#.into())).unwrap();
    assert_eq!(
        chains_to(graph, "react", &[]),
        ["app@0.0.0-use.local > table@3.2.5 > react@18.3.1"]
    );
}

#[test]
fn pnpm_reads_the_project_document_when_pnpm_prepends_its_own() {
    let lock = "lockfileVersion: '9.0'\nimporters:\n  .:\n    configDependencies: {}\n---\nlockfileVersion: '9.0'\nimporters:\n  .:\n    dependencies:\n      left-pad:\n        specifier: ^1.0.0\n        version: 1.3.0\npackages:\n  left-pad@1.3.0: {}\nsnapshots:\n  left-pad@1.3.0: {}\n";
    let graph = lockfiles::pnpm(lock, &|_| None).unwrap();
    assert_eq!(
        workspace_dependencies(graph, &[]),
        ["<.>@0.0.0-use.local prod left-pad@1.3.0 ^1.0.0"]
    );
}

#[test]
fn package_lock_resolves_a_nested_copy_before_the_hoisted_one() {
    let lock = r#"{
      "lockfileVersion": 3,
      "packages": {
        "": { "name": "app", "dependencies": { "a": "^1.0.0", "b": "^2.0.0" } },
        "node_modules/a": { "version": "1.0.0", "dependencies": { "b": "^1.0.0" } },
        "node_modules/a/node_modules/b": { "version": "1.5.0" },
        "node_modules/b": { "version": "2.1.0" }
      }
    }"#;
    let walk = Walk::new(lockfiles::package_lock(lock).unwrap());
    let a = walk.nodes_named("a")[0];
    assert_eq!(
        described(&walk, walk.dependencies(a, &[])),
        ["a@1.0.0 prod b@1.5.0 ^1.0.0"]
    );
}

#[test]
fn a_cargo_workspace_crate_takes_ranges_from_its_manifest_including_inherited_ones() {
    let lock = "version = 4\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"syn\"]\n[[package]]\nname = \"syn\"\nversion = \"1.0.109\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n";
    let mut manifests = CargoManifests::default();
    manifests.add(".", &|_| Some("[workspace.dependencies]\nsyn = \"1.0\"\n[package]\nname = \"app\"\n[dependencies]\nsyn = { workspace = true }\n".into()));
    let graph = lockfiles::cargo(lock, &manifests).unwrap();
    assert_eq!(
        workspace_dependencies(graph, &[]),
        ["app@0.1.0 prod syn@1.0.109 1.0"]
    );
}

#[test]
fn two_lockfiles_with_same_named_root_packages_stay_two_workspaces() {
    let mut graph = Graph::default();
    graph.merge(yarn_app(), ".");
    graph.merge(yarn_app(), "docs");
    let workspaces: Vec<_> = graph
        .nodes
        .iter()
        .filter_map(|n| n.workspace.clone())
        .collect();
    assert_eq!(workspaces, [".", "docs"]);
}

#[test]
fn registry_releases_are_described_oldest_first_with_dependencies_grouped_by_kind() {
    let release = |version: &str| Release {
        version: version.into(),
        yanked: false,
        dependencies: vec![
            ("dnd".into(), "^13".into(), Kind::Prod),
            ("react".into(), "^18".into(), Kind::Peer),
        ],
    };
    let described = describe(
        Ecosystem::Npm,
        "table",
        &[release("3.10.0"), release("3.9.0-rc.1")],
    );
    assert_eq!(
        described["versions"][0],
        json!({
            "version": "3.9.0-rc.1", "yanked": false, "prerelease": true,
            "dependencies": { "dnd": "^13" }, "peerDependencies": { "react": "^18" }, "optionalDependencies": {}
        })
    );
}

const PACKAGE_LOCK: &str = r#"{
  "name": "app",
  "lockfileVersion": 3,
  "packages": {
    "": { "name": "app", "version": "1.0.0", "dependencies": { "a": "^1.0.0" } },
    "node_modules/a": { "version": "1.0.0" }
  }
}"#;

fn repository(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in files {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

const YARN_V1: &str = "# yarn lockfile v1\n\na@^1:\n  version \"1.0.0\"\n";

#[test]
fn a_lockfile_that_cannot_be_read_is_skipped_when_none_were_named() {
    let dir = repository(&[("package-lock.json", PACKAGE_LOCK), ("yarn.lock", YARN_V1)]);
    let graph = lockfiles::load(dir.path(), &[]).unwrap();
    assert_eq!(graph.lockfiles, vec!["package-lock.json".to_string()]);
    assert_eq!(graph.skipped.len(), 1);
    assert_eq!(graph.skipped[0].lockfile, "yarn.lock");
    assert_eq!(graph.skipped[0].error, "Yarn v1 lockfiles are not read yet");
}

#[test]
fn a_named_lockfile_that_cannot_be_read_fails() {
    let dir = repository(&[
        ("package-lock.json", PACKAGE_LOCK),
        ("examples/old/yarn.lock", YARN_V1),
    ]);
    let error = lockfiles::load(dir.path(), &["examples/**/yarn.lock".to_string()]).unwrap_err();
    assert!(error.contains("Yarn v1"), "{error}");
}

#[test]
fn loading_fails_when_no_lockfile_can_be_read() {
    let dir = repository(&[("yarn.lock", YARN_V1)]);
    let error = lockfiles::load(dir.path(), &[]).unwrap_err();
    assert!(error.contains("no lockfile could be read"), "{error}");
    assert!(error.contains("yarn.lock: Yarn v1"), "{error}");
}

#[test]
fn only_root_lockfiles_are_read_when_none_were_named() {
    let dir = repository(&[
        ("package-lock.json", PACKAGE_LOCK),
        ("packages/web/package-lock.json", PACKAGE_LOCK),
        ("examples/old/yarn.lock", YARN_V1),
    ]);
    let graph = lockfiles::load(dir.path(), &[]).unwrap();
    assert_eq!(graph.lockfiles, vec!["package-lock.json".to_string()]);
    assert!(graph.skipped.is_empty());
}

#[test]
fn nested_lockfiles_alone_point_to_a_glob_for_the_lockfile_found() {
    let dir = repository(&[("packages/web/package-lock.json", PACKAGE_LOCK)]);
    let error = lockfiles::load(dir.path(), &[]).unwrap_err();
    assert!(error.contains("**/package-lock.json"), "{error}");
}

#[test]
fn a_double_star_glob_also_matches_the_root_lockfile() {
    let dir = repository(&[
        ("package-lock.json", PACKAGE_LOCK),
        ("packages/web/package-lock.json", PACKAGE_LOCK),
    ]);
    let graph = lockfiles::load(dir.path(), &["**/package-lock.json".to_string()]).unwrap();
    assert_eq!(
        graph.lockfiles,
        vec![
            "package-lock.json".to_string(),
            "packages/web/package-lock.json".to_string()
        ]
    );
}

#[test]
fn a_pnpm_project_that_is_not_a_workspace_is_its_own_root() {
    let lock = r#"
lockfileVersion: '6.0'
dependencies:
  react:
    specifier: ^18.2.0
    version: 18.2.0
devDependencies:
  typescript:
    specifier: ^5.0.0
    version: 5.4.5
packages:
  /react@18.2.0:
    resolution: {integrity: sha512-x}
  /typescript@5.4.5:
    resolution: {integrity: sha512-y}
"#;
    let graph = lockfiles::pnpm(lock, &|path| {
        (path == "package.json").then(|| r#"{ "name": "app" }"#.to_string())
    })
    .unwrap();
    let walk = Walk::new(graph);
    let root = walk
        .graph
        .nodes
        .iter()
        .position(|n| n.name == "app")
        .expect("a root workspace");
    let mut declared: Vec<String> = walk
        .dependencies(root, &[])
        .into_iter()
        .map(|edge| {
            let edge = &walk.graph.edges[edge];
            let to = edge.to.map(|to| walk.graph.nodes[to].version.clone());
            format!("{} {:?} {:?} {:?}", edge.name, edge.kind, edge.range, to)
        })
        .collect();
    declared.sort();
    assert_eq!(
        declared,
        vec![
            "react Prod Some(\"^18.2.0\") Some(\"18.2.0\")".to_string(),
            "typescript Dev Some(\"^5.0.0\") Some(\"5.4.5\")".to_string(),
        ]
    );
}

#[test]
fn a_pnpm_importer_with_a_key_that_is_not_text_is_ignored() {
    let lock = "lockfileVersion: '9.0'\nimporters:\n  ~:\n    dependencies: {}\n  .:\n    dependencies: {}\n";
    assert!(lockfiles::pnpm(lock, &|_| None).is_ok());
}

#[test]
fn an_aliased_npm_install_is_the_package_it_aliases() {
    let lock = r#"{
      "lockfileVersion": 3,
      "packages": {
        "": { "name": "app", "dependencies": { "foo": "npm:bar@^1.0.0" } },
        "node_modules/foo": { "name": "bar", "version": "1.2.0" }
      }
    }"#;
    let walk = Walk::new(lockfiles::package_lock(lock).unwrap());
    assert_eq!(walk.nodes_named("bar").len(), 1);
    assert!(walk.nodes_named("foo").is_empty());
}

#[test]
fn an_optional_cargo_dependency_inherited_from_the_workspace_stays_optional() {
    let lock = "version = 4\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"serde\"]\n[[package]]\nname = \"serde\"\nversion = \"1.0.200\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n";
    let mut manifests = CargoManifests::default();
    manifests.add(".", &|_| Some("[workspace.dependencies]\nserde = \"1.0\"\n[package]\nname = \"app\"\n[dependencies]\nserde = { workspace = true, optional = true }\n".into()));
    let graph = lockfiles::cargo(lock, &manifests).unwrap();
    assert_eq!(
        workspace_dependencies(graph, &[]),
        ["app@0.1.0 optional serde@1.0.200 1.0"]
    );
}

#[test]
fn cargo_manifests_outside_the_workspace_members_are_not_taken() {
    let dir = repository(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        (
            "crates/app/Cargo.toml",
            "[package]\nname = \"app\"\n[dependencies]\nsyn = \"2\"\n",
        ),
        (
            "tests/fixtures/app/Cargo.toml",
            "[package]\nname = \"app\"\n[dependencies]\nsyn = \"1\"\n",
        ),
    ]);
    let manifests = CargoManifests::find(dir.path());
    assert_eq!(manifests.members["app"].0, "crates/app");
}

#[test]
fn a_registry_answer_that_cannot_be_read_is_an_error_not_an_empty_package() {
    use codemod_deps::registry::{crate_releases, npm_releases};
    assert!(npm_releases("<html>busy</html>").is_err());
    assert!(npm_releases("[]").is_err());
    assert_eq!(
        npm_releases(r#"{ "name": "a", "versions": {} }"#)
            .unwrap()
            .len(),
        0
    );
    assert!(crate_releases("{\"vers\":\"1.0.0\"}\n{\"vers\":").is_err());
    assert_eq!(crate_releases("{\"vers\":\"1.0.0\"}\n\n").unwrap().len(), 1);
}

#[test]
fn a_range_with_a_protocol_other_than_npm_cannot_be_read() {
    use codemod_deps::semver::satisfies;
    assert_eq!(satisfies(Ecosystem::Npm, "file:../foo@^1", "1.2.0"), None);
    assert_eq!(satisfies(Ecosystem::Npm, "link:../foo", "1.2.0"), None);
    assert_eq!(satisfies(Ecosystem::Npm, "npm:foo@^1", "1.2.0"), Some(true));
    assert_eq!(satisfies(Ecosystem::Npm, "^1", "2.0.0"), Some(false));
}

#[test]
fn a_registry_package_does_not_merge_into_a_workspace_with_its_name_and_version() {
    let mut local = Graph::from_lockfile("Cargo.lock");
    local.workspace(Ecosystem::Cargo, "foo", "1.0.0", ".");
    let mut vendored = Graph::from_lockfile("Cargo.lock");
    vendored.node(Ecosystem::Cargo, "foo", "1.0.0");
    let mut graph = Graph::default();
    graph.merge(local, ".");
    graph.merge(vendored, "tools");
    let foos: Vec<_> = graph
        .nodes
        .iter()
        .filter(|n| n.name == "foo")
        .map(|n| n.workspace.clone())
        .collect();
    assert_eq!(foos, [Some(".".to_string()), None]);
}

#[test]
fn package_lock_looks_in_every_ancestor_directory_before_the_root() {
    let lock = r#"{
      "lockfileVersion": 3,
      "packages": {
        "": { "name": "root" },
        "packages/app": { "name": "app", "dependencies": { "x": "^2.0.0" } },
        "node_modules/app": { "resolved": "packages/app", "link": true },
        "packages/node_modules/x": { "version": "2.0.0" },
        "node_modules/x": { "version": "1.0.0" }
      }
    }"#;
    let graph = lockfiles::package_lock(lock).unwrap();
    assert_eq!(
        chains_to(graph, "x", &[]),
        ["app@0.0.0-use.local > x@2.0.0"]
    );
}

#[test]
fn a_pnpm_peer_installed_with_a_package_is_a_peer_edge_not_a_prod_one() {
    let lock = r#"
lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      table:
        specifier: ^3.0.0
        version: 3.2.5(react@18.3.1)
packages:
  table@3.2.5:
    peerDependencies:
      react: ^18
  react@18.3.1: {}
snapshots:
  table@3.2.5(react@18.3.1):
    dependencies:
      react: 18.3.1
  react@18.3.1: {}
"#;
    let graph = lockfiles::pnpm(lock, &|_| Some(r#"{ "name": "app" }"#.into())).unwrap();
    assert_eq!(
        chains_to(graph, "react", &[Kind::Prod]),
        Vec::<String>::new()
    );
}

#[path = "specs/python.rs"]
mod python;

#[path = "specs/php.rs"]
mod php;
