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
        ["app@workspace > table@3.2.5 > react@18.3.1"]
    );
}

#[test]
fn pnpm_reads_the_project_document_when_pnpm_prepends_its_own() {
    let lock = "lockfileVersion: '9.0'\nimporters:\n  .:\n    configDependencies: {}\n---\nlockfileVersion: '9.0'\nimporters:\n  .:\n    dependencies:\n      left-pad:\n        specifier: ^1.0.0\n        version: 1.3.0\npackages:\n  left-pad@1.3.0: {}\nsnapshots:\n  left-pad@1.3.0: {}\n";
    let graph = lockfiles::pnpm(lock, &|_| None).unwrap();
    assert_eq!(
        workspace_dependencies(graph, &[]),
        ["<.>@workspace prod left-pad@1.3.0 ^1.0.0"]
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
