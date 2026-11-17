use depq::model::{Eco, Kind};
use depq::parse::{cargo, pnpm, yarn, CargoManifests};
use depq::query::{first_release_accepting, parse_pattern, run, Hits, Verdict};
use depq::registry::{Registry, Release};
use std::collections::HashMap;

const YARN_LOCK: &str = r#"
__metadata:
  version: 8

"app@workspace:.":
  version: 0.0.0-use.local
  resolution: "app@workspace:."
  dependencies:
    table: "npm:^3.0.0"
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

const APP_PACKAGE_JSON: &str = r#"{ "name": "app", "dependencies": { "table": "^3.0.0" }, "devDependencies": { "dnd": "^13.0.0" } }"#;

fn backstage_like() -> depq::model::Graph {
    yarn(YARN_LOCK, &|p| (p == "package.json").then(|| APP_PACKAGE_JSON.to_string())).unwrap()
}

fn edges(h: Hits) -> Vec<String> {
    match h {
        Hits::Edges(e) => e.into_iter().map(|e| format!("{} {} {}", e.from, e.kind, e.to)).collect(),
        Hits::Paths(_) => panic!("expected edges"),
    }
}

fn paths(h: Hits) -> Vec<String> {
    match h {
        Hits::Paths(p) => p.into_iter().map(|p| p.chain.join(" > ")).collect(),
        Hits::Edges(_) => panic!("expected paths"),
    }
}

#[test]
fn a_workspace_dependency_kind_comes_from_its_package_json() {
    let g = backstage_like();
    let p = parse_pattern("ws:* -[dev]-> npm:*").unwrap();
    assert_eq!(edges(run(&g, &p)), ["app@0.0.0-use.local dev dnd@13.0.0"]);
}

#[test]
fn a_path_query_returns_the_shortest_chain_from_each_workspace() {
    let mut g = backstage_like();
    // Without the dev edge, dnd arrives through table.
    g.edges.retain(|e| e.kind != Kind::Dev);
    let p = parse_pattern("ws:* ~> npm:dnd").unwrap();
    assert_eq!(paths(run(&g, &p)), ["app@0.0.0-use.local > table@3.2.5 > dnd@13.0.0"]);
}

#[test]
fn refuses_lists_peer_ranges_that_exclude_the_version() {
    let g = backstage_like();
    let p = parse_pattern("ext:* -[peer]-> npm:react refuses 19.0.0").unwrap();
    assert_eq!(edges(run(&g, &p)), ["table@3.2.5 peer react", "dnd@13.0.0 peer react"]);
}

#[test]
fn accepts_is_the_complement_of_refuses() {
    let g = backstage_like();
    let p = parse_pattern("ext:* -[peer]-> npm:react accepts 18.3.1").unwrap();
    assert_eq!(edges(run(&g, &p)).len(), 2);
}

#[test]
fn pnpm_collapses_peer_suffixed_snapshots_into_one_node() {
    let lock = r#"
lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      ui:
        specifier: ^1.0.0
        version: 1.0.0(react@18.3.1)
packages:
  ui@1.0.0:
    peerDependencies:
      react: ^18
  react@18.3.1: {}
snapshots:
  ui@1.0.0(react@18.3.1):
    dependencies:
      react: 18.3.1
  ui@1.0.0(react@19.0.0):
    dependencies:
      react: 18.3.1
  react@18.3.1: {}
"#;
    let g = pnpm(lock, &|_| Some(r#"{"name":"root"}"#.into())).unwrap();
    assert_eq!(g.nodes.iter().filter(|n| n.name == "ui").count(), 1);
}

#[test]
fn a_cargo_dependency_with_a_version_resolves_to_that_version() {
    let lock = r#"
version = 4
[[package]]
name = "app"
version = "0.1.0"
dependencies = ["bitflags 1.3.2"]

[[package]]
name = "bitflags"
version = "1.3.2"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "bitflags"
version = "2.10.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
    let g = cargo(lock, &CargoManifests { members: HashMap::new(), root: None }).unwrap();
    let p = parse_pattern("ws:* ~> cargo:bitflags@^1").unwrap();
    assert_eq!(paths(run(&g, &p)), ["app@0.1.0 > bitflags@1.3.2"]);
}

struct FakeRegistry(Vec<Release>);
impl Registry for FakeRegistry {
    fn releases(&self, _: Eco, _: &str) -> Vec<Release> {
        self.0.clone()
    }
}

fn release(version: &str, react_peer: &str) -> Release {
    Release { version: version.into(), yanked: false, deps: vec![("react".into(), react_peer.into(), Kind::Peer)] }
}

#[test]
fn a_later_release_that_accepts_the_version_is_the_fix() {
    let reg = FakeRegistry(vec![release("6.0.1", "^16 || ^17 || ^18"), release("8.0.0", "^16 || ^17 || ^18 || ^19")]);
    let v = first_release_accepting(&reg, Eco::Npm, "react-inspector", "6.0.1", "react", "19.0.0");
    assert_eq!(v, Verdict::Fixable { version: "8.0.0".into(), breaking: true });
}

#[test]
fn a_package_with_no_accepting_release_is_stuck() {
    let reg = FakeRegistry(vec![release("4.12.4", "^16.8.0 || ^17.0.0")]);
    let v = first_release_accepting(&reg, Eco::Npm, "@material-ui/core", "4.12.4", "react", "19.0.0");
    assert_eq!(v, Verdict::Stuck { latest: Some("4.12.4".into()) });
}

#[test]
fn a_zero_major_minor_bump_counts_as_breaking() {
    let reg = FakeRegistry(vec![release("0.30.0", "^19")]);
    let v = first_release_accepting(&reg, Eco::Npm, "@graphiql/react", "0.29.0", "react", "19.0.0");
    assert_eq!(v, Verdict::Fixable { version: "0.30.0".into(), breaking: true });
}

#[test]
fn a_path_pattern_rejects_refuses() {
    assert!(parse_pattern("ws:* ~> npm:react refuses 19.0.0").is_err());
}

#[test]
fn a_refusal_the_installed_release_does_not_publish_is_a_local_override() {
    let published = Release { version: "4.1.12".into(), yanked: false, deps: vec![("vite".into(), "^6.0.0 || ^7.0.0 || ^8.0.0".into(), Kind::Peer)] };
    let v = first_release_accepting(&FakeRegistry(vec![published]), Eco::Npm, "vitest", "4.1.12", "vite", "8.0.0");
    assert_eq!(v, Verdict::LocalOverride);
}
