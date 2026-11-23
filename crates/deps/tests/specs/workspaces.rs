//! Chains that cross from one of the repository's own packages to another.

use super::*;

/// The demo monorepo: web -> @demo/shared -> moment, api -> moment.
const DEMO_LOCK: &str = include_str!("../fixtures/npm-workspaces/package-lock.json");

fn demo() -> Graph {
    lockfiles::package_lock(DEMO_LOCK).unwrap()
}

#[test]
fn a_package_lock_chain_crosses_a_workspace_the_start_depends_on() {
    assert_eq!(
        chains_to(demo(), "moment", &[]),
        [
            "@demo/api@1.0.0 > moment@2.29.4",
            "@demo/shared@1.0.0 > moment@2.29.4",
            "@demo/web@1.0.0 > @demo/shared@1.0.0 > moment@2.29.4",
        ]
    );
}

#[test]
fn a_chain_through_a_workspace_keeps_to_the_kinds_asked_for() {
    // web -> shared is prod, shared -> moment is dev, api -> moment is prod.
    let lock = r#"
lockfileVersion: '9.0'
importers:
  .: {}
  packages/api:
    dependencies:
      moment:
        specifier: ^2.29.4
        version: 2.29.4
  packages/shared:
    devDependencies:
      moment:
        specifier: ^2.29.4
        version: 2.29.4
  packages/web:
    dependencies:
      '@demo/shared':
        specifier: workspace:*
        version: link:../shared
packages:
  moment@2.29.4: {}
snapshots:
  moment@2.29.4: {}
"#;
    let graph = || {
        lockfiles::pnpm(lock, &|path| match path {
            "packages/api/package.json" => Some(r#"{ "name": "@demo/api" }"#.into()),
            "packages/shared/package.json" => Some(r#"{ "name": "@demo/shared" }"#.into()),
            "packages/web/package.json" => Some(r#"{ "name": "@demo/web" }"#.into()),
            _ => None,
        })
        .unwrap()
    };
    assert_eq!(
        chains_to(graph(), "moment", &[Kind::Prod]),
        ["@demo/api@0.0.0-use.local > moment@2.29.4"]
    );
    assert_eq!(
        chains_to(graph(), "moment", &[]),
        [
            "@demo/api@0.0.0-use.local > moment@2.29.4",
            "@demo/shared@0.0.0-use.local > moment@2.29.4",
            "@demo/web@0.0.0-use.local > @demo/shared@0.0.0-use.local > moment@2.29.4",
        ]
    );
}

#[test]
fn chains_to_nothing_are_empty() {
    let walk = Walk::new(demo());
    assert!(walk
        .chains(&walk.workspaces(), &HashSet::new(), &[])
        .is_empty());
}

#[test]
fn a_pnpm_chain_crosses_a_linked_workspace() {
    let lock = r#"
lockfileVersion: '9.0'
importers:
  .: {}
  packages/shared:
    dependencies:
      moment:
        specifier: ^2.29.4
        version: 2.29.4
  packages/web:
    dependencies:
      '@demo/shared':
        specifier: workspace:*
        version: link:../shared
packages:
  moment@2.29.4: {}
snapshots:
  moment@2.29.4: {}
"#;
    let graph = lockfiles::pnpm(lock, &|path| match path {
        "packages/shared/package.json" => Some(r#"{ "name": "@demo/shared" }"#.into()),
        "packages/web/package.json" => Some(r#"{ "name": "@demo/web" }"#.into()),
        _ => None,
    })
    .unwrap();
    assert_eq!(
        chains_to(graph, "moment", &[]),
        [
            "@demo/shared@0.0.0-use.local > moment@2.29.4",
            "@demo/web@0.0.0-use.local > @demo/shared@0.0.0-use.local > moment@2.29.4",
        ]
    );
}

#[test]
fn a_yarn_chain_crosses_a_workspace_protocol_dependency() {
    let lock = r#"
__metadata:
  version: 8

"@demo/shared@workspace:*, @demo/shared@workspace:packages/shared":
  version: 0.0.0-use.local
  resolution: "@demo/shared@workspace:packages/shared"
  dependencies:
    moment: "npm:^2.29.4"
  languageName: unknown
  linkType: soft

"@demo/web@workspace:packages/web":
  version: 0.0.0-use.local
  resolution: "@demo/web@workspace:packages/web"
  dependencies:
    "@demo/shared": "workspace:*"
  languageName: unknown
  linkType: soft

"moment@npm:^2.29.4":
  version: 2.29.4
  resolution: "moment@npm:2.29.4"
  languageName: node
  linkType: hard
"#;
    let graph = lockfiles::yarn(lock, &|path| match path {
        "packages/shared/package.json" => {
            Some(r#"{ "name": "@demo/shared", "dependencies": { "moment": "^2.29.4" } }"#.into())
        }
        "packages/web/package.json" => Some(
            r#"{ "name": "@demo/web", "dependencies": { "@demo/shared": "workspace:*" } }"#.into(),
        ),
        _ => None,
    })
    .unwrap();
    assert_eq!(
        chains_to(graph, "moment", &[]),
        [
            "@demo/shared@0.0.0-use.local > moment@2.29.4",
            "@demo/web@0.0.0-use.local > @demo/shared@0.0.0-use.local > moment@2.29.4",
        ]
    );
}

#[test]
fn a_cargo_chain_crosses_a_path_dependency_on_another_member() {
    let lock = "version = 4\n[[package]]\nname = \"web\"\nversion = \"0.1.0\"\ndependencies = [\"shared\"]\n[[package]]\nname = \"shared\"\nversion = \"0.1.0\"\ndependencies = [\"chrono\"]\n[[package]]\nname = \"chrono\"\nversion = \"0.4.38\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n";
    let mut manifests = CargoManifests::default();
    manifests.add("web", &|_| {
        Some(
            "[package]\nname = \"web\"\n[dependencies]\nshared = { path = \"../shared\" }\n".into(),
        )
    });
    manifests.add("shared", &|_| {
        Some("[package]\nname = \"shared\"\n[dependencies]\nchrono = \"0.4\"\n".into())
    });
    let graph = lockfiles::cargo(lock, &manifests).unwrap();
    assert_eq!(
        chains_to(graph, "chrono", &[]),
        [
            "web@0.1.0 > shared@0.1.0 > chrono@0.4.38",
            "shared@0.1.0 > chrono@0.4.38",
        ]
    );
}

#[test]
fn a_uv_chain_crosses_an_editable_workspace_member() {
    let lock = r#"
version = 1

[[package]]
name = "app"
version = "0.1.0"
source = { editable = "." }
dependencies = [{ name = "lib" }]

[package.metadata]
requires-dist = [{ name = "lib", editable = "packages/lib" }]

[[package]]
name = "lib"
version = "0.2.0"
source = { editable = "packages/lib" }
dependencies = [{ name = "six" }]

[package.metadata]
requires-dist = [{ name = "six", specifier = ">=1.16" }]

[[package]]
name = "six"
version = "1.16.0"
source = { registry = "https://pypi.org/simple" }
"#;
    assert_eq!(
        chains_to(lockfiles::uv(lock).unwrap(), "six", &[]),
        [
            "app@0.1.0 > lib@0.2.0 > six@1.16.0",
            "lib@0.2.0 > six@1.16.0"
        ]
    );
}
