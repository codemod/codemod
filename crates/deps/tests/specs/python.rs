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
            "app@0.1.0 prod lib@0.2.0 *",
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
    let walk = Walk::new(lockfiles::poetry(lock, &|_| None).unwrap());
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
