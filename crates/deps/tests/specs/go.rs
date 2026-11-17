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
