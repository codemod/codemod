use super::*;

const GO_MOD: &str = r#"
module example.com/app

go 1.22

require (
	github.com/BurntSushi/toml v1.4.0
	golang.org/x/text v0.21.0 // indirect
)
"#;

fn read_none(_: &str) -> Option<String> {
    None
}

/// A reader over `files`, as paths relative to the lockfile's directory.
fn files(files: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
    move |path| {
        let path = path.trim_start_matches("./");
        files
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, text)| text.to_string())
    }
}

#[test]
fn every_go_mod_requirement_indirect_ones_included_is_a_prod_edge_ranged_at_its_version() {
    assert_eq!(
        workspace_dependencies(lockfiles::go_mod(GO_MOD, &read_none).unwrap(), &[]),
        [
            "example.com/app@0.0.0-use.local prod github.com/BurntSushi/toml@v1.4.0 v1.4.0",
            "example.com/app@0.0.0-use.local prod golang.org/x/text@v0.21.0 v0.21.0"
        ]
    );
}

#[test]
fn the_module_is_the_workspace_at_the_go_mod_directory() {
    let graph = lockfiles::go_mod(GO_MOD, &read_none).unwrap();
    let workspaces: Vec<_> = graph
        .nodes
        .iter()
        .filter_map(|n| Some((n.name.as_str(), n.workspace.as_deref()?)))
        .collect();
    assert_eq!(workspaces, [("example.com/app", ".")]);
}

#[test]
fn a_go_module_path_keeps_its_case() {
    let walk = Walk::new(lockfiles::go_mod(GO_MOD, &read_none).unwrap());
    assert_eq!(
        (
            walk.nodes_named("github.com/BurntSushi/toml").len(),
            walk.nodes_named("github.com/burntsushi/toml").len()
        ),
        (1, 0)
    );
}

#[test]
fn a_replaced_module_installs_the_replacement_under_the_required_name() {
    let go_mod = "module example.com/app\n\nrequire github.com/old/thing v1.0.0\n\nreplace github.com/old/thing v1.0.0 => github.com/new/thing v1.2.0\n";
    assert_eq!(
        workspace_dependencies(lockfiles::go_mod(go_mod, &read_none).unwrap(), &[]),
        ["example.com/app@0.0.0-use.local prod github.com/new/thing@v1.2.0 v1.0.0"]
    );
}

#[test]
fn a_directory_replacement_is_a_workspace_whose_requirements_resolve_to_the_main_modules_versions()
{
    let go_mod = "module example.com/app\n\nrequire (\n\texample.com/lib v0.0.0\n\tgolang.org/x/text v0.21.0\n)\n\nreplace example.com/lib => ./lib\n";
    let read = files(&[(
        "lib/go.mod",
        "module example.com/lib\n\nrequire golang.org/x/text v0.3.0\n",
    )]);
    assert_eq!(
        chains_to(
            lockfiles::go_mod(go_mod, &read).unwrap(),
            "golang.org/x/text",
            &[]
        ),
        [
            "example.com/app@0.0.0-use.local > golang.org/x/text@v0.21.0",
            "example.com/lib@0.0.0-use.local > golang.org/x/text@v0.21.0"
        ]
    );
}

const GO_WORK_FILES: &[(&str, &str)] = &[
    ("go.work", "go 1.22\n\nuse (\n\t.\n\t./lib\n)\n"),
    (
        "go.mod",
        "module example.com/app\n\nrequire example.com/lib v0.0.0\n",
    ),
    ("lib/go.mod", "module example.com/lib\n"),
];

#[test]
fn a_requirement_on_a_go_work_module_resolves_to_that_workspace() {
    let graph = lockfiles::go_work(GO_WORK_FILES[0].1, &files(GO_WORK_FILES)).unwrap();
    assert_eq!(
        workspace_dependencies(graph, &[]),
        ["example.com/app@0.0.0-use.local prod example.com/lib@0.0.0-use.local v0.0.0"]
    );
}

#[test]
fn a_go_mod_beside_a_go_work_reads_as_the_workspace() {
    let dir = repository(GO_WORK_FILES);
    let graph = lockfiles::load(dir.path(), &[]).unwrap();
    assert_eq!(graph.lockfiles, ["go.mod", "go.work"]);
    assert_eq!(
        workspace_dependencies(graph, &[]),
        ["example.com/app@0.0.0-use.local prod example.com/lib@0.0.0-use.local v0.0.0"]
    );
}

#[test]
fn a_go_mod_without_a_module_directive_cannot_be_read() {
    assert_eq!(
        lockfiles::go_mod("go 1.22\n", &read_none).unwrap_err(),
        "no module directive"
    );
}

#[test]
fn a_go_requirement_accepts_any_version_at_or_above_it_on_the_same_module_path() {
    use codemod_deps::semver::satisfies;
    let check = |range, version| satisfies(Ecosystem::Go, range, version);
    assert_eq!(
        [
            check("v1.4.0", "v1.10.0"),
            check("v1.4.0", "v1.3.9"),
            check("v0.9.0", "v1.0.0"),
            check("v1.4.0", "v2.0.0"),
            check("v17.0.0+incompatible", "v20.10.7+incompatible"),
        ],
        [Some(true), Some(false), Some(true), Some(false), Some(true)]
    );
}

#[test]
fn a_go_pseudo_version_sorts_before_the_release_it_precedes() {
    use codemod_deps::semver::compare;
    assert!(compare(
        Ecosystem::Go,
        "v1.2.4-0.20240101000000-abcdef123456",
        "v1.2.4"
    )
    .is_lt());
}

#[test]
fn a_string_that_is_not_a_go_version_sorts_before_every_version() {
    use codemod_deps::semver::compare;
    let mut versions = ["v1.10.0", "master", "v1.2.0", "latest"];
    versions.sort_by(|a, b| compare(Ecosystem::Go, a, b));
    assert_eq!(versions, ["latest", "master", "v1.2.0", "v1.10.0"]);
}

#[test]
fn only_a_major_change_breaks_a_go_module() {
    use codemod_deps::semver::is_breaking;
    assert_eq!(
        [
            is_breaking(Ecosystem::Go, "v0.29.0", "v0.30.0"),
            is_breaking(Ecosystem::Go, "v1.9.0", "v2.0.0")
        ],
        [false, true]
    );
}

#[test]
fn a_go_release_the_latest_go_mod_retracts_is_yanked() {
    use codemod_deps::registry::go_releases;
    let list = "v1.14.53\nv2.0.0+incompatible\nv2.0.3+incompatible\n";
    let latest = "module github.com/mattn/go-sqlite3\n\nretract (\n\t[v2.0.0+incompatible, v2.0.7+incompatible] // Accidental\n)\n";
    let yanked: Vec<_> = go_releases(list, Some(latest))
        .unwrap()
        .into_iter()
        .map(|release| (release.version, release.yanked))
        .collect();
    assert_eq!(
        yanked,
        [
            ("v1.14.53".to_string(), false),
            ("v2.0.0+incompatible".to_string(), true),
            ("v2.0.3+incompatible".to_string(), true)
        ]
    );
}

#[test]
fn a_go_version_list_with_a_line_that_is_not_a_version_cannot_be_read() {
    use codemod_deps::registry::go_releases;
    assert!(go_releases("not found: module example.com/x", None).is_err());
}
