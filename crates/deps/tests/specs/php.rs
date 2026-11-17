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

fn composer_check(range: &str, version: &str) -> Option<bool> {
    codemod_deps::semver::satisfies(Ecosystem::Packagist, range, version)
}

#[test]
fn composer_carets_tildes_wildcards_and_hyphen_ranges_follow_composer() {
    assert_eq!(
        [
            composer_check("^1.2.3", "1.9.0"),
            composer_check("^0.3", "0.4.0"),
            composer_check("~1.2", "1.9.0"),
            composer_check("~1.2.3", "1.3.0"),
            composer_check("1.2.*", "v1.2.9"),
            composer_check("1.0 - 2.1", "2.1.5"),
            composer_check("1.0.0 - 2.1.0", "2.1.5"),
            composer_check(">= 1.0, < 1.1 || ^2.0", "2.3.0"),
            composer_check("^2.0 | ^3.0", "1.0.0"),
        ],
        [
            Some(true),
            Some(false),
            Some(true),
            Some(false),
            Some(true),
            Some(true),
            Some(false),
            Some(true),
            Some(false)
        ]
    );
}

#[test]
fn a_composer_bound_without_a_stability_starts_at_that_versions_dev_release() {
    assert_eq!(
        [
            composer_check("<2.0", "2.0.0-beta1"),
            composer_check("^1.0", "2.0.0-alpha1"),
            composer_check(">=2.0", "2.0.0-RC1"),
        ],
        [Some(false), Some(false), Some(true)]
    );
}

#[test]
fn stability_flags_are_ignored_and_an_alias_is_read_from_its_left_side() {
    assert_eq!(
        [
            composer_check("^1.0@beta", "1.2.0"),
            composer_check("@dev", "3.0.0"),
            composer_check("2.x-dev as 2.1.0", "2.x-dev"),
        ],
        [Some(true), Some(true), Some(true)]
    );
}

#[test]
fn a_composer_dev_branch_cannot_be_compared() {
    assert_eq!(
        [
            composer_check("^1.0", "dev-main"),
            composer_check("dev-main", "1.0.0")
        ],
        [None, None]
    );
}

#[test]
fn composer_versions_order_by_stability_with_patches_after_the_release() {
    use codemod_deps::semver::compare;
    let mut versions = [
        "1.0.0-patch1",
        "v1.0.1",
        "1.0.0",
        "1.0.0-RC1",
        "1.0.0-beta2",
        "1.0.0-alpha1",
        "1.0.0-dev",
    ];
    versions.sort_by(|a, b| compare(Ecosystem::Packagist, a, b));
    assert_eq!(
        versions,
        [
            "1.0.0-dev",
            "1.0.0-alpha1",
            "1.0.0-beta2",
            "1.0.0-RC1",
            "1.0.0",
            "1.0.0-patch1",
            "v1.0.1"
        ]
    );
}

#[test]
fn composer_branches_sort_before_every_version() {
    use codemod_deps::semver::compare;
    let mut versions = ["1.10.0", "dev-main", "1.2.0", "dev-feature"];
    versions.sort_by(|a, b| compare(Ecosystem::Packagist, a, b));
    assert_eq!(versions, ["dev-feature", "dev-main", "1.2.0", "1.10.0"]);
}

#[test]
fn a_composer_package_is_found_by_its_name_in_any_case() {
    let walk = Walk::new(composer_app());
    let found: Vec<_> = walk
        .nodes_named("Monolog/Monolog")
        .into_iter()
        .map(|id| walk.graph.label(id))
        .collect();
    assert_eq!(found, ["monolog/monolog@3.8.1"]);
}

#[test]
fn a_composer_patch_release_is_not_a_prerelease() {
    use codemod_deps::semver::is_prerelease;
    assert_eq!(
        [
            is_prerelease(Ecosystem::Packagist, "1.0.0-RC1"),
            is_prerelease(Ecosystem::Packagist, "1.0.0-patch1")
        ],
        [true, false]
    );
}
