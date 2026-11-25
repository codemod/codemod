//! `codemod/deps`: `dependencies()` and `install()` on fixture
//! repositories (tests/fixtures/deps): every example in the spec
//! (~/codemod/drafts/deps-api-spec.md) and the docs runs here.

mod common;

use std::path::Path;

use common::{output, Harness};
use serde_json::{json, Value};

/// A harness whose target is a copy of `tests/fixtures/deps/<name>`.
fn fixture(name: &str) -> Harness {
    let h = Harness::new();
    let from = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/deps")
        .join(name);
    copy(&from, h.target.path());
    h
}

fn copy(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// What `body` (statements, with `deps` = dependencies()) returns from one function step.
fn ask(h: &Harness, body: &str) -> Result<Value, String> {
    let source = format!(
        r#"
import {{ step }} from "codemod/workflow";
import {{ dependencies, semver }} from "codemod/deps";
export default async function main() {{
  return await step(() => {{
    const deps = dependencies();
    {body}
  }});
}}
"#
    );
    output(&h.start(&source, Value::Null))
}

fn value(h: &Harness, expression: &str) -> Value {
    ask(h, &format!("return {expression};")).unwrap()
}

// ── the evidence questions, one line each ─────────────────────────────────

#[test]
fn does_the_repo_use_x() {
    let h = fixture("npm-workspaces");
    assert_eq!(
        value(&h, r#"[deps.uses("moment"), deps.uses("left-pad")]"#),
        json!([true, false])
    );
}

#[test]
fn does_the_package_that_owns_this_file_use_x() {
    let h = fixture("no-lockfile");
    assert_eq!(
        value(
            &h,
            r#"deps.packageOf("apps/web/src/app/page.tsx")?.uses("next", { kinds: "prod" })"#
        ),
        json!(true)
    );
    // The same from a forEachFile callback, per file.
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { dependencies } from "codemod/deps";
export default async function main() {
  const found = await step(forEachFile("apps/**/*.tsx", (file) =>
    dependencies().packageOf(file.path)?.uses("next", { kinds: "prod" })));
  return found.outputs;
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!([true]));
}

#[test]
fn bump_x_where_declared() {
    let h = fixture("npm-workspaces");
    let edits = value(&h, r#"deps.bump("moment", "^2.30.1")"#);
    assert_eq!(
        edits,
        json!([
            { "file": "packages/api/package.json", "package": "packages/api", "name": "moment", "kind": "prod", "from": "^2.29.4", "to": "^2.30.1" },
            { "file": "packages/shared/package.json", "package": "packages/shared", "name": "moment", "kind": "prod", "from": "^2.29.4", "to": "^2.30.1" },
        ])
    );
    assert_eq!(
        h.read("packages/shared/package.json"),
        "{\n  \"name\": \"@demo/shared\",\n  \"version\": \"1.0.0\",\n  \"dependencies\": {\n    \"moment\": \"^2.30.1\"\n  }\n}\n"
    );
}

#[test]
fn bump_through_a_pnpm_catalog() {
    let h = fixture("pnpm-catalogs");
    let edits = value(&h, r#"deps.bump("react", "^19.0.0")"#);
    // web declares react from the catalog; shared's peer range is its own.
    assert_eq!(
        edits,
        json!([
            { "file": "packages/shared/package.json", "package": "packages/shared", "name": "react", "kind": "peer", "from": ">=17", "to": "^19.0.0" },
            { "file": "pnpm-workspace.yaml", "package": null, "name": "react", "kind": "catalog", "from": "^18.2.0", "to": "^19.0.0" },
        ])
    );
    assert!(h
        .read("pnpm-workspace.yaml")
        .contains("catalog:\n  react: ^19.0.0\n"));
    assert!(h
        .read("packages/web/package.json")
        .contains("\"react\": \"catalog:\""));
}

#[test]
fn bump_only_some_kinds_or_with_a_function() {
    let h = fixture("pnpm-catalogs");
    let edits = value(
        &h,
        r#"deps.bump("lodash", (d) => semver.major(d.resolved) === 3 ? "^4.17.21" : undefined)"#,
    );
    assert_eq!(edits.as_array().unwrap().len(), 1);
    assert!(h
        .read("packages/shared/package.json")
        .contains("\"lodash\": \"^4.17.21\""));
    let edits = value(
        &h,
        r#"deps.bump("react", "^19.0.0", { kinds: ["prod", "dev"] })"#,
    );
    assert_eq!(edits[0]["kind"], json!("catalog"));
    assert!(h
        .read("packages/shared/package.json")
        .contains("\"react\": \">=17\""));
}

#[test]
fn bump_through_a_bun_catalog() {
    let h = fixture("bun-catalogs");
    let edits = value(&h, r#"deps.bump("react", "^19.0.0")"#);
    assert_eq!(
        edits,
        json!([{ "file": "package.json", "package": null, "name": "react", "kind": "catalog", "from": "^18.2.0", "to": "^19.0.0" }])
    );
    assert!(h
        .read("package.json")
        .contains("\"catalog\": {\n      \"react\": \"^19.0.0\"\n    }"));
    assert!(h
        .read("package.json")
        .contains("\"dependencies\": {\n    \"react\": \"catalog:\"\n  }"));
}

#[test]
fn add_remove_rename() {
    let h = fixture("single");
    value(&h, r#"deps.add("vitest", "^3.0.0", { kind: "dev" })"#);
    value(&h, r#"deps.remove("chalk")"#);
    assert_eq!(
        h.read("package.json"),
        "{\n  \"name\": \"single\",\n  \"version\": \"1.0.0\",\n  \"dependencies\": {},\n  \"devDependencies\": {\n    \"mocha\": \"^10.0.0\",\n    \"vitest\": \"^3.0.0\"\n  }\n}\n"
    );

    let h = fixture("yarn-berry");
    let edits = value(
        &h,
        r#"deps.rename("react-router-dom", "react-router", { range: "^7.0.0" })"#,
    );
    assert_eq!(
        edits,
        json!([{ "file": "packages/app/package.json", "package": "packages/app", "name": "react-router", "kind": "prod", "from": "^6.20.0", "to": "^7.0.0", "renamedFrom": "react-router-dom" }])
    );
    assert!(h
        .read("packages/app/package.json")
        .contains("\"express\": \"^4.18.0\",\n    \"react-router\": \"^7.0.0\"\n"));
}

#[test]
fn add_next_to_an_existing_dependency_in_the_same_section() {
    let h = fixture("yarn-berry");
    value(
        &h,
        r#"deps.declarations("express").flatMap((d) => d.package.add("mime-types", "^3.0.0", { kind: d.kind }))"#,
    );
    assert!(h.read("packages/app/package.json").contains(
        "\"express\": \"^4.18.0\",\n    \"mime-types\": \"^3.0.0\",\n    \"react-router-dom\""
    ));
}

#[test]
fn which_version_declared_and_installed() {
    let h = fixture("pnpm-catalogs");
    assert_eq!(
        value(&h, r#"deps.declarations("react").map((d) => d.resolved)"#),
        json!([">=17", "^18.2.0"])
    );
    assert_eq!(
        value(&h, r#"deps.installed("lodash")"#),
        json!(["3.10.1", "4.17.21"])
    );
    let h = fixture("npm-workspaces");
    assert_eq!(value(&h, r#"deps.installed("moment")"#), json!(["2.29.4"]));
    assert_eq!(
        value(
            &h,
            r#"deps.package("packages/api").declaration("moment").installed()"#
        ),
        json!("moment@2.29.4")
    );
}

#[test]
fn range_checks_without_hand_rolled_regexes() {
    let h = fixture("no-lockfile");
    assert_eq!(
        value(
            &h,
            r#"[
              deps.declarations("zod").some((d) => semver.major(d.resolved ?? "") === 3),
              deps.declarations("zod").map((d) => d.accepts("4.1.0")),
              semver.lt(deps.declarations("zod")[0].minVersion()!, "5.0.0"),
              semver.coerce("pnpm@10.1.3"),
              semver.maxSatisfying(["3.22.0", "3.23.8", "4.0.0"], "^3.22.0"),
            ]"#
        ),
        json!([true, [false, true, true], true, "10.1.3", "3.23.8"])
    );
}

#[test]
fn which_package_manager() {
    for (name, expected) in [
        (
            "npm-workspaces",
            json!(["npm", null, "package-lock.json", "lockfile"]),
        ),
        (
            "pnpm-catalogs",
            json!(["pnpm", "9.12.0", "pnpm-lock.yaml", "packageManager"]),
        ),
        (
            "yarn-berry",
            json!(["yarn", "4.5.0", "yarn.lock", "packageManager"]),
        ),
        ("bun-catalogs", json!(["bun", null, "bun.lock", "lockfile"])),
        (
            "single",
            json!(["npm", null, "package-lock.json", "lockfile"]),
        ),
        ("no-lockfile", json!(["npm", null, null, "default"])),
    ] {
        let h = fixture(name);
        let pm = value(&h, "deps.packageManager");
        assert_eq!(
            json!([
                pm["name"],
                pm["version"],
                pm["lockfile"],
                pm["detectedFrom"]
            ]),
            expected,
            "{name}"
        );
    }
    assert_eq!(
        value(&fixture("pnpm-catalogs"), "deps.packageManager?.name"),
        json!("pnpm")
    );
}

#[test]
fn which_of_my_packages_declare_x_and_in_which_section() {
    let h = fixture("npm-workspaces");
    assert_eq!(
        value(
            &h,
            r#"deps.declarations("moment").map((d) => d.package.dir)"#
        ),
        json!(["packages/api", "packages/shared"])
    );
    let h = fixture("no-lockfile");
    assert_eq!(
        value(
            &h,
            r#"deps.declarations("zod").filter((d) => d.kind !== "dev").map((d) => [d.package.dir, d.kind])"#
        ),
        json!([["apps/web", "prod"], ["packages/lib", "peer"]])
    );
}

#[test]
fn list_my_packages_and_find_the_owner_of_a_file() {
    let h = fixture("npm-workspaces");
    assert_eq!(
        value(&h, "deps.packages().map((p) => p.name)"),
        json!(["workflow-v2-demo", "@demo/api", "@demo/shared", "@demo/web"])
    );
    assert_eq!(
        value(&h, "deps.packages()"),
        json!([
            "workflow-v2-demo@0.0.0",
            "@demo/api@1.0.0",
            "@demo/shared@1.0.0",
            "@demo/web@1.0.0"
        ])
    );
    let h = fixture("no-lockfile");
    assert_eq!(
        value(&h, r#"deps.packageOf("apps/web/src/app/page.tsx")?.dir"#),
        json!("apps/web")
    );
    assert_eq!(value(&h, r#"deps.packageOf("README.md")?.dir"#), json!("."));
}

#[test]
fn what_catalog_and_workspace_refs_resolve_to() {
    let h = fixture("pnpm-catalogs");
    assert_eq!(
        value(
            &h,
            r#"deps.declarations("react", { kinds: "prod" }).map((d) => [d.range, d.resolved, d.catalog])"#
        ),
        json!([["catalog:", "^18.2.0", { "name": "default", "file": "pnpm-workspace.yaml" }]])
    );
    assert_eq!(
        value(
            &h,
            r#"deps.declarations("moment").map((d) => [d.range, d.resolved, d.catalog.name])"#
        ),
        json!([["catalog:legacy", "2.29.4", "legacy"]])
    );
    assert_eq!(
        value(
            &h,
            r#"(({ range, resolved, protocol, local }) => [range, resolved, protocol, local.dir])(deps.package("@demo/web").declaration("@demo/shared"))"#
        ),
        json!(["workspace:^", "^1.2.0", "workspace", "packages/shared"])
    );
    let h = fixture("bun-catalogs");
    assert_eq!(
        value(
            &h,
            r#"deps.declarations("react").map((d) => [d.package.dir, d.resolved, d.catalog.file])"#
        ),
        json!([
            [".", "^18.2.0", "package.json"],
            ["packages/ui", "^18.2.0", "package.json"]
        ])
    );
}

#[test]
fn which_ranges_can_be_read() {
    let h = fixture("no-lockfile");
    assert_eq!(
        value(
            &h,
            r#"deps.package("apps/web").declarations().map((d) => [d.name, d.protocol, d.readable])"#
        ),
        json!([
            ["@acme/lib", "workspace", true],
            ["next", "registry", true],
            ["ui-kit", "other", false],
            ["zod", "registry", true],
        ])
    );
}

#[test]
fn needs_install_compares_the_manifests_with_the_lockfile() {
    for name in [
        "npm-workspaces",
        "pnpm-catalogs",
        "yarn-berry",
        "bun-catalogs",
        "single",
    ] {
        assert_eq!(
            value(&fixture(name), "deps.needsInstall()"),
            json!(false),
            "{name}"
        );
    }
    assert_eq!(
        value(&fixture("no-lockfile"), "deps.needsInstall()"),
        json!(true)
    );
    let h = fixture("single");
    assert_eq!(
        value(
            &h,
            r#"[deps.needsInstall(), deps.bump("chalk", "^5.0.0").length, deps.needsInstall()]"#
        ),
        json!([false, 1, true])
    );
    // The next step reads the edited manifest from disk: still behind the lockfile.
    assert_eq!(value(&h, "deps.needsInstall()"), json!(true));
}

// ── graph ─────────────────────────────────────────────────────────────────

#[test]
fn chains_to_x() {
    let h = fixture("npm-workspaces");
    assert_eq!(
        value(
            &h,
            r#"deps.chains({ to: "moment" }).map((c) => c.join(" > "))"#
        ),
        json!([
            "@demo/api@1.0.0 > moment@2.29.4",
            "@demo/shared@1.0.0 > moment@2.29.4",
            "@demo/web@1.0.0 > @demo/shared@1.0.0 > moment@2.29.4",
        ])
    );
    // A name that is not installed reaches nothing; a package can start a chain.
    assert_eq!(value(&h, r#"deps.chains({ to: "left-pad" })"#), json!([]));
    assert_eq!(
        value(
            &h,
            r#"deps.chains({ from: deps.package("@demo/web"), to: "moment" })"#
        ),
        json!([["@demo/web@1.0.0", "@demo/shared@1.0.0", "moment@2.29.4"]])
    );
}

#[test]
fn why_is_x_installed() {
    let h = fixture("pnpm-catalogs");
    assert_eq!(
        value(
            &h,
            r#"deps.nodes("lodash").map((n) => [String(n), n.installedBy({ includeThirdParty: true }).map((e) => String(e.from))])"#
        ),
        json!([
            ["lodash@3.10.1", ["@demo/shared@1.2.0"]],
            ["lodash@4.17.21", ["@demo/web@1.0.0"]]
        ])
    );
}

#[test]
fn dependents_of_my_package() {
    let h = fixture("npm-workspaces");
    assert_eq!(
        value(
            &h,
            r#"deps.dependents("@demo/shared").map((e) => e.from.dir)"#
        ),
        json!(["packages/api", "packages/web"])
    );
    let h = fixture("no-lockfile");
    assert_eq!(
        value(
            &h,
            r#"deps.declarations("@acme/lib").map((d) => d.package.dir)"#
        ),
        json!(["apps/web"])
    );
}

// ── semantics ─────────────────────────────────────────────────────────────

#[test]
fn the_graph_loads_on_first_use_and_stays_as_loaded_after_an_edit() {
    let h = fixture("single");
    assert_eq!(
        value(
            &h,
            r#"({
              before: deps.installed("chalk"),
              edited: deps.bump("chalk", "^5.0.0").map((e) => e.to),
              declared: deps.declarations("chalk").map((d) => d.range),
              installed: deps.installed("chalk"),
              lockfile: deps.lockfiles,
            })"#
        ),
        json!({ "before": ["4.1.2"], "edited": ["^5.0.0"], "declared": ["^5.0.0"], "installed": ["4.1.2"], "lockfile": ["package-lock.json"] })
    );
}

#[test]
fn reads_work_without_a_lockfile_and_the_graph_is_empty() {
    let h = fixture("no-lockfile");
    assert_eq!(
        value(
            &h,
            r#"[deps.packages().length, deps.lockfiles, deps.installed("zod"), deps.chains({ to: "zod" })]"#
        ),
        json!([3, [], [], []])
    );
}

#[test]
fn nodes_and_packages_in_a_step_result_are_recorded_as_text() {
    let h = fixture("npm-workspaces");
    assert_eq!(
        ask(&h, r#"return { nodes: deps.nodes("moment"), declaration: deps.declarations("moment")[0] };"#).unwrap(),
        json!({
            "nodes": ["moment@2.29.4"],
            "declaration": { "package": "packages/api", "name": "moment", "range": "^2.29.4", "kind": "prod", "resolved": "^2.29.4" },
        })
    );
}

#[test]
fn a_dry_run_shows_the_edit_to_later_steps_and_writes_nothing() {
    let h = fixture("single");
    let before = h.read("package.json");
    let report = h.dry_run(
        r#"
import { step } from "codemod/workflow";
import { dependencies, install } from "codemod/deps";
export default async function main() {
  await step(() => dependencies().bump("chalk", "^5.0.0"));
  const installed = await step(install());
  const range = await step(() => dependencies().declarations("chalk")[0].range);
  return { installed, range };
}
"#,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({
            "installed": { "packageManager": "npm", "command": "npm install --package-lock-only --no-audit --no-fund", "ran": false, "lockfile": "package-lock.json", "changed": false },
            "range": "^5.0.0",
        })
    );
    assert_eq!(h.read("package.json"), before);
}

#[test]
fn dependencies_runs_inside_steps_and_install_in_the_body() {
    let h = fixture("single");
    let report = h.start(
        r#"
import { dependencies } from "codemod/deps";
export default async function main() { return dependencies().packages(); }
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(
        error.contains("dependencies() must run inside a step"),
        "{error}"
    );

    let source = r#"
import { step } from "codemod/workflow";
import { install } from "codemod/deps";
export default async function main() { await step(() => install()); }
"#;
    let path = h.bundle.path().join("workflow.ts");
    std::fs::write(&path, source).unwrap();
    let error = codemod_workflow::load_workflow(&path)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(
        error.contains("uses 'install' inside a function step"),
        "{error}"
    );
}

#[test]
fn install_needs_a_package_json() {
    let h = Harness::new();
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { install } from "codemod/deps";
export default async function main() { await step(install()); }
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("install: no package.json in ."), "{error}");
}

#[test]
fn dependency_graph_is_a_build_error_that_says_what_to_write() {
    let h = fixture("single");
    let path = h.bundle.path().join("workflow.ts");
    std::fs::write(
        &path,
        r#"
import { step } from "codemod/workflow";
import { dependencyGraph } from "codemod/deps";
export default async function main() { await step(dependencyGraph((g) => g.workspaces())); }
"#,
    )
    .unwrap();
    let error = codemod_workflow::load_workflow(&path)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(
        error.contains("dependencyGraph() was removed: call dependencies()"),
        "{error}"
    );
}
