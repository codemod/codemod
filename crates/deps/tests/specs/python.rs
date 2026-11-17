use super::*;

const UV_LOCK: &str = r#"
version = 1
requires-python = ">=3.12"

[manifest]
members = ["app", "lib"]

[[package]]
name = "app"
version = "0.1.0"
source = { editable = "." }
dependencies = [{ name = "lib" }, { name = "requests" }]

[package.optional-dependencies]
socks = [{ name = "pysocks" }]

[package.dev-dependencies]
dev = [{ name = "pytest" }]

[package.metadata]
requires-dist = [
    { name = "lib", editable = "packages/lib" },
    { name = "pysocks", marker = "extra == 'socks'", specifier = ">=1.7" },
    { name = "requests", specifier = ">=2.31" },
]

[package.metadata.requires-dev]
dev = [{ name = "pytest", specifier = ">=8" }]

[[package]]
name = "lib"
version = "0.2.0"
source = { editable = "packages/lib" }

[[package]]
name = "pysocks"
version = "1.7.1"
source = { registry = "https://pypi.org/simple" }

[[package]]
name = "pytest"
version = "8.3.4"
source = { registry = "https://pypi.org/simple" }

[[package]]
name = "requests"
version = "2.32.3"
source = { registry = "https://pypi.org/simple" }
dependencies = [{ name = "certifi" }]

[[package]]
name = "certifi"
version = "2024.8.30"
source = { registry = "https://pypi.org/simple" }
"#;

#[test]
fn a_uv_workspace_takes_ranges_and_kinds_from_the_lock_metadata() {
    assert_eq!(
        workspace_dependencies(lockfiles::uv(UV_LOCK).unwrap(), &[]),
        [
            "app@0.1.0 prod lib@0.2.0 ?",
            "app@0.1.0 prod requests@2.32.3 >=2.31",
            "app@0.1.0 optional pysocks@1.7.1 >=1.7",
            "app@0.1.0 dev pytest@8.3.4 >=8"
        ]
    );
}

#[test]
fn uv_workspace_members_are_workspaces_in_their_own_directories() {
    let graph = lockfiles::uv(UV_LOCK).unwrap();
    let workspaces: Vec<_> = graph
        .nodes
        .iter()
        .filter_map(|n| n.workspace.clone())
        .collect();
    assert_eq!(workspaces, [".", "packages/lib"]);
}

#[test]
fn a_third_party_dependency_in_uv_lock_has_no_range() {
    let walk = Walk::new(lockfiles::uv(UV_LOCK).unwrap());
    assert_eq!(
        described(&walk, walk.declaring("certifi", &[])),
        ["requests@2.32.3 prod certifi@2024.8.30 ?"]
    );
}

const POETRY_LOCK: &str = r#"
[[package]]
name = "requests"
version = "2.32.3"
optional = false
python-versions = ">=3.8"

[package.dependencies]
certifi = ">=2017.4.17"
PySocks = {version = ">=1.5.6,!=1.5.7", optional = true}

[[package]]
name = "certifi"
version = "2024.8.30"

[[package]]
name = "pysocks"
version = "1.7.1"

[[package]]
name = "typing-extensions"
version = "4.12.2"

[[package]]
name = "pytest"
version = "8.3.4"

[metadata]
lock-version = "2.1"
python-versions = "^3.12"
"#;

const PYPROJECT: &str = r#"
[project]
name = "App"
version = "1.0.0"
dependencies = ["requests[socks] >=2.31 ; python_version < '3.14'"]

[tool.poetry.dependencies]
python = "^3.12"
typing_extensions = "^4.12"

[tool.poetry.group.test.dependencies]
pytest = "^8"
"#;

fn poetry_app() -> Graph {
    lockfiles::poetry(POETRY_LOCK, &|path| {
        (path == "pyproject.toml").then(|| PYPROJECT.to_string())
    })
    .unwrap()
}

#[test]
fn a_poetry_project_takes_its_dependencies_from_pyproject_with_groups_as_dev() {
    assert_eq!(
        workspace_dependencies(poetry_app(), &[]),
        [
            "app@1.0.0 prod requests@2.32.3 >=2.31",
            "app@1.0.0 prod typing-extensions@4.12.2 ^4.12",
            "app@1.0.0 dev pytest@8.3.4 ^8"
        ]
    );
}

#[test]
fn an_optional_dependency_in_poetry_lock_is_an_optional_edge_with_its_range() {
    let walk = Walk::new(poetry_app());
    assert_eq!(
        described(&walk, walk.declaring("pysocks", &[])),
        ["requests@2.32.3 optional pysocks@1.7.1 >=1.5.6,!=1.5.7"]
    );
}

const PIPFILE_LOCK: &str = r#"{
  "_meta": { "pipfile-spec": 6 },
  "default": {
    "django": { "version": "==5.1.3" },
    "asgiref": { "version": "==3.8.1" }
  },
  "develop": {
    "pytest": { "version": "==8.3.4" }
  }
}"#;

#[test]
fn pipfile_lock_edges_come_only_from_what_the_pipfile_declares() {
    let pipfile =
        "[packages]\nDjango = \"~=5.1\"\n\n[dev-packages]\npytest = { version = \"*\" }\n";
    let graph = lockfiles::pipfile(PIPFILE_LOCK, &|path| {
        (path == "Pipfile").then(|| pipfile.to_string())
    })
    .unwrap();
    assert_eq!(
        workspace_dependencies(graph, &[]),
        [
            "<.>@0.0.0-use.local prod django@5.1.3 ~=5.1",
            "<.>@0.0.0-use.local dev pytest@8.3.4 *"
        ]
    );
}

#[test]
fn a_poetry_lock_is_read_with_the_pyproject_beside_it() {
    let dir = repository(&[("poetry.lock", POETRY_LOCK), ("pyproject.toml", PYPROJECT)]);
    let walk = Walk::new(lockfiles::load(dir.path(), &[]).unwrap());
    let roots: Vec<_> = walk
        .workspaces()
        .iter()
        .map(|&id| walk.graph.label(id))
        .collect();
    assert_eq!(roots, ["app@1.0.0"]);
}

#[test]
fn poetry_carets_tildes_and_alternatives_read_as_pep_440_ranges() {
    use codemod_deps::semver::satisfies;
    let check = |range, version| satisfies(Ecosystem::Pypi, range, version);
    assert_eq!(
        [
            check("^8", "9.0"),
            check("~1.2", "1.2.9"),
            check(">=1.5.6,<1.5.7 || >1.5.7", "1.5.7"),
            check("~=5.1", "5.2.19"),
        ],
        [Some(false), Some(true), Some(false), Some(true)]
    );
}

#[test]
fn a_python_range_that_is_not_pep_440_cannot_be_read() {
    use codemod_deps::semver::satisfies;
    assert_eq!(satisfies(Ecosystem::Pypi, "latest", "1.0"), None);
}

#[test]
fn a_pypi_release_is_yanked_only_when_every_file_is() {
    use codemod_deps::registry::pypi_releases;
    let document = r#"{ "releases": {
        "1.0": [{ "yanked": true }, { "yanked": false }],
        "1.1": [{ "yanked": true }]
    } }"#;
    let yanked: Vec<_> = pypi_releases(document)
        .unwrap()
        .into_iter()
        .map(|release| (release.version, release.yanked))
        .collect();
    assert_eq!(
        yanked,
        [("1.0".to_string(), false), ("1.1".to_string(), true)]
    );
}

#[test]
fn poetry_carets_and_tildes_take_prerelease_and_epoch_versions() {
    use codemod_deps::semver::satisfies;
    let check = |range, version| satisfies(Ecosystem::Pypi, range, version);
    assert_eq!(
        [
            check("^1.2rc1", "1.3"),
            check("~1.2rc1", "1.3"),
            check("^1!1.2", "1!1.9"),
            check("^1!1.2", "1.9"),
        ],
        [Some(true), Some(false), Some(true), Some(false)]
    );
}

#[test]
fn a_poetry_bound_too_large_to_raise_cannot_be_read() {
    use codemod_deps::semver::satisfies;
    assert_eq!(
        satisfies(Ecosystem::Pypi, "^18446744073709551615", "1.0"),
        None
    );
}

#[test]
fn a_python_prerelease_reaching_its_release_is_not_breaking() {
    use codemod_deps::semver::is_breaking;
    assert_eq!(
        [
            is_breaking(Ecosystem::Pypi, "0.1rc1", "0.1"),
            is_breaking(Ecosystem::Pypi, "0.1", "0.2"),
            is_breaking(Ecosystem::Pypi, "1.9", "2.0rc1"),
        ],
        [false, true, true]
    );
}

#[test]
fn a_poetry_dependency_resolves_to_the_locked_version_its_range_allows() {
    let lock = r#"
[[package]]
name = "app-lib"
version = "1.0"
dependencies = { foo = ">=2" }

[[package]]
name = "foo"
version = "1.0"

[[package]]
name = "foo"
version = "2.0"
"#;
    let walk = Walk::new(lockfiles::poetry(lock, &|_| Some(String::new())).unwrap());
    assert_eq!(
        described(&walk, walk.declaring("foo", &[])),
        ["app-lib@1.0 prod foo@2.0 >=2"]
    );
}

#[test]
fn a_pyproject_dependency_split_by_markers_keeps_every_range() {
    let pyproject = r#"
[tool.poetry]
name = "app"
version = "1.0.0"

[tool.poetry.dependencies]
foo = [{ version = "^1", python = "<3.10" }, { version = "^2", python = ">=3.10" }]
"#;
    let lock = "[[package]]\nname = \"foo\"\nversion = \"2.1\"\n";
    let graph = lockfiles::poetry(lock, &|path| {
        (path == "pyproject.toml").then(|| pyproject.to_string())
    })
    .unwrap();
    assert_eq!(
        workspace_dependencies(graph, &[]),
        ["app@1.0.0 prod foo@2.1 ^1 || ^2"]
    );
}

#[test]
fn a_pypi_version_with_no_files_is_not_a_release() {
    use codemod_deps::registry::pypi_releases;
    let document = r#"{ "releases": { "1.0": [{ "yanked": false }], "9.0": [] } }"#;
    let versions: Vec<_> = pypi_releases(document)
        .unwrap()
        .into_iter()
        .map(|release| release.version)
        .collect();
    assert_eq!(versions, ["1.0"]);
}

#[test]
fn versions_pypi_cannot_read_sort_before_the_ones_it_can() {
    use codemod_deps::semver::compare;
    // Long enough that an order that is not total would make the sort panic.
    let mut versions: Vec<String> = (0..60)
        .flat_map(|n| {
            [
                format!("{n}.0"),
                format!("{n}-legacy"),
                format!("{n}.0.post1"),
            ]
        })
        .collect();
    versions.sort_by(|a, b| compare(Ecosystem::Pypi, a, b));
    assert_eq!(
        versions[..3].to_vec(),
        ["0-legacy", "1-legacy", "10-legacy"]
    );
    assert_eq!(versions[60..62].to_vec(), ["0.0", "0.0.post1"]);
}

#[test]
fn a_python_package_is_found_by_any_spelling_pypi_accepts() {
    let walk = Walk::new(poetry_app());
    let found: Vec<_> = walk
        .nodes_named("Typing_Extensions")
        .into_iter()
        .map(|id| walk.graph.label(id))
        .collect();
    assert_eq!(found, ["typing-extensions@4.12.2"]);
}

#[test]
fn a_git_or_path_dependency_in_pyproject_has_no_range() {
    let pyproject = r#"
[tool.poetry]
name = "app"
version = "1.0.0"

[tool.poetry.dependencies]
foo = { git = "https://example.com/foo.git" }
bar = { path = "../bar", develop = true }
"#;
    let graph = lockfiles::poetry("", &|_| Some(pyproject.to_string())).unwrap();
    assert_eq!(
        workspace_dependencies(graph, &[]),
        ["app@1.0.0 prod bar ?", "app@1.0.0 prod foo ?"]
    );
}

#[test]
fn poetry_ranges_can_join_clauses_with_spaces_and_alternatives_with_one_bar() {
    use codemod_deps::semver::satisfies;
    let check = |range, version| satisfies(Ecosystem::Pypi, range, version);
    assert_eq!(
        [
            check(">=1.2 <2.0", "2.1"),
            check(">= 1.2, < 2.0", "1.5"),
            check(">=1.2 | <0.5", "0.4"),
        ],
        [Some(false), Some(true), Some(true)]
    );
}

#[test]
fn python_versions_that_name_the_same_release_are_not_breaking() {
    use codemod_deps::semver::is_breaking;
    assert_eq!(
        [
            is_breaking(Ecosystem::Pypi, "0.0", "0.0.0"),
            is_breaking(Ecosystem::Pypi, "1.0", "v1.0"),
            is_breaking(Ecosystem::Pypi, "1.0", "1!1.0"),
        ],
        [false, false, true]
    );
}

const PIPFILE_LOCK_SOURCES: &str = r#"{
  "_meta": { "pipfile-spec": 6 },
  "default": {
    "app": { "editable": true, "path": "." },
    "foo": { "git": "https://example.com/foo.git", "ref": "abc123" },
    "bar": { "path": "./vendor/bar-1.0-py3-none-any.whl" }
  },
  "docs": {
    "sphinx": { "version": "==8.1.3" }
  }
}"#;

fn pipfile_sources() -> Graph {
    let pipfile = "[packages]\napp = { editable = true, path = \".\" }\nfoo = { git = \"https://example.com/foo.git\" }\nbar = { path = \"./vendor/bar-1.0-py3-none-any.whl\" }\n\n[docs]\nsphinx = \"*\"\n";
    lockfiles::pipfile(PIPFILE_LOCK_SOURCES, &|path| {
        (path == "Pipfile").then(|| pipfile.to_string())
    })
    .unwrap()
}

#[test]
fn a_pipfile_project_installed_as_editable_is_the_root() {
    let walk = Walk::new(pipfile_sources());
    let roots: Vec<_> = walk
        .workspaces()
        .iter()
        .map(|&id| walk.graph.label(id))
        .collect();
    assert_eq!(roots, ["app@0.0.0-use.local"]);
}

#[test]
fn pipfile_git_and_file_sources_are_packages_with_no_range() {
    assert_eq!(
        workspace_dependencies(pipfile_sources(), &[Kind::Prod]),
        [
            "app@0.0.0-use.local prod bar@0.0.0-use.local ?",
            "app@0.0.0-use.local prod foo@abc123 ?"
        ]
    );
}

#[test]
fn a_custom_pipfile_category_is_a_dev_dependency() {
    assert_eq!(
        workspace_dependencies(pipfile_sources(), &[Kind::Dev]),
        ["app@0.0.0-use.local dev sphinx@8.1.3 *"]
    );
}

#[test]
fn a_poetry_lock_without_its_pyproject_cannot_be_read() {
    assert!(lockfiles::poetry(POETRY_LOCK, &|_| None).is_err());
}

#[test]
fn pypi_release_versions_are_normalized_as_lockfiles_write_them() {
    use codemod_deps::registry::pypi_releases;
    let document = r#"{ "releases": { "2.0.0.RC1": [{ "yanked": false }] } }"#;
    let versions: Vec<_> = pypi_releases(document)
        .unwrap()
        .into_iter()
        .map(|release| release.version)
        .collect();
    assert_eq!(versions, ["2.0.0rc1"]);
}

#[test]
fn a_pypi_release_lists_no_dependency_groups() {
    let release = Release {
        version: "1.0".into(),
        yanked: false,
        dependencies: Vec::new(),
    };
    let described = describe(Ecosystem::Pypi, "foo", &[release]);
    assert_eq!(
        described["versions"][0],
        json!({ "version": "1.0", "yanked": false, "prerelease": false })
    );
}
