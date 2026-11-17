use super::*;

const COMPOSER_LOCK: &str = r#"{
  "packages": [
    {
      "name": "Acme/Lib",
      "version": "dev-main",
      "dist": { "type": "path", "url": "packages/lib" },
      "require": { "php": ">=8.1", "psr/log": "^3.0" },
      "require-dev": { "psr/clock": "^1" }
    },
    {
      "name": "monolog/monolog",
      "version": "3.8.1",
      "dist": { "type": "zip", "url": "https://example.test/monolog.zip" },
      "require": { "php": ">=8.1", "psr/log": "^2.0 || ^3.0" },
      "provide": { "psr/log-implementation": "3.0.0" }
    },
    {
      "name": "psr/log",
      "version": "3.0.2",
      "require": { "php": ">=8.0.0" }
    }
  ],
  "packages-dev": [
    {
      "name": "acme/logger-test",
      "version": "v1.0.0",
      "require": { "psr/log-implementation": "*" }
    }
  ]
}"#;

const COMPOSER_JSON: &str = r#"{
  "name": "Acme/App",
  "require": { "php": "^8.1", "ext-json": "*", "monolog/monolog": "^3.5", "acme/lib": "@dev" },
  "require-dev": { "acme/logger-test": "^1.0" }
}"#;

fn composer_app() -> Graph {
    lockfiles::composer(COMPOSER_LOCK, &|path| {
        (path == "composer.json").then(|| COMPOSER_JSON.to_string())
    })
    .unwrap()
}

#[test]
fn a_composer_project_takes_ranges_and_kinds_from_composer_json_without_platform_requirements() {
    let walk = Walk::new(composer_app());
    let root = walk
        .graph
        .nodes
        .iter()
        .position(|n| n.workspace.as_deref() == Some("."));
    assert_eq!(
        described(&walk, walk.dependencies(root.unwrap(), &[])),
        [
            "acme/app@0.0.0-use.local prod acme/lib@dev-main @dev",
            "acme/app@0.0.0-use.local prod monolog/monolog@3.8.1 ^3.5",
            "acme/app@0.0.0-use.local dev acme/logger-test@v1.0.0 ^1.0"
        ]
    );
}

#[test]
fn a_locked_composer_package_requires_its_dependencies_as_prod_edges_with_constraints() {
    let walk = Walk::new(composer_app());
    assert_eq!(
        described(&walk, walk.declaring("psr/log", &[])),
        [
            "acme/lib@dev-main prod psr/log@3.0.2 ^3.0",
            "monolog/monolog@3.8.1 prod psr/log@3.0.2 ^2.0 || ^3.0"
        ]
    );
}

#[test]
fn a_path_repository_package_is_a_workspace_in_its_directory() {
    let graph = composer_app();
    let workspaces: Vec<_> = graph
        .nodes
        .iter()
        .filter_map(|n| Some((n.name.as_str(), n.workspace.as_deref()?)))
        .collect();
    assert_eq!(
        workspaces,
        [("acme/lib", "packages/lib"), ("acme/app", ".")]
    );
}

#[test]
fn a_requirement_on_a_provided_package_resolves_to_the_package_providing_it() {
    let walk = Walk::new(composer_app());
    assert_eq!(
        described(&walk, walk.declaring("psr/log-implementation", &[])),
        ["acme/logger-test@v1.0.0 prod monolog/monolog@3.8.1 *"]
    );
}

#[test]
fn a_composer_lock_is_read_with_the_composer_json_beside_it() {
    let dir = repository(&[
        ("composer.lock", COMPOSER_LOCK),
        ("composer.json", COMPOSER_JSON),
    ]);
    let walk = Walk::new(lockfiles::load(dir.path(), &[]).unwrap());
    let roots: Vec<_> = walk
        .workspaces()
        .iter()
        .map(|&id| walk.graph.label(id))
        .collect();
    assert_eq!(roots, ["acme/lib@dev-main", "acme/app@0.0.0-use.local"]);
}
