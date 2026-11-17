use std::path::Path;

use super::*;

const GRADLE_LOCKFILE: &str = "\
# This is a Gradle generated file for dependency locking.
# Manual edits can break the build and are not advised.
# This file is expected to be part of source control.
com.google.guava:guava:31.1-jre=compileClasspath,runtimeClasspath,testCompileClasspath,testRuntimeClasspath
org.junit.jupiter:junit-jupiter-api:5.9.0=testCompileClasspath,testRuntimeClasspath
org.projectlombok:lombok:1.18.30=annotationProcessor,compileClasspath
com.google.dagger:dagger-compiler:2.51=kapt
empty=testAnnotationProcessor
";

fn gradle_app(files: &[(&str, &str)]) -> Graph {
    lockfiles::gradle(
        GRADLE_LOCKFILE,
        &|path| {
            files
                .iter()
                .find(|(name, _)| *name == path)
                .map(|(_, text)| text.to_string())
        },
        Path::new("app"),
    )
    .unwrap()
}

#[test]
fn every_locked_module_is_an_edge_from_the_project_with_no_range() {
    assert_eq!(
        workspace_dependencies(gradle_app(&[]), &[]),
        [
            "app@0.0.0-use.local prod com.google.guava:guava@31.1-jre ?",
            "app@0.0.0-use.local dev org.junit.jupiter:junit-jupiter-api@5.9.0 ?",
            "app@0.0.0-use.local prod org.projectlombok:lombok@1.18.30 ?",
            "app@0.0.0-use.local build com.google.dagger:dagger-compiler@2.51 ?"
        ]
    );
}

#[test]
fn a_module_on_main_and_test_classpaths_is_one_prod_edge() {
    let walk = Walk::new(gradle_app(&[]));
    assert_eq!(
        described(&walk, walk.declaring("com.google.guava:guava", &[])),
        ["app@0.0.0-use.local prod com.google.guava:guava@31.1-jre ?"]
    );
}

#[test]
fn the_gradle_project_takes_its_name_from_settings_and_its_version_from_gradle_properties() {
    let walk = Walk::new(gradle_app(&[
        ("settings.gradle.kts", "rootProject.name = \"shop\"\n"),
        ("gradle.properties", "version=2.4.0\n"),
    ]));
    assert_eq!(walk.graph.label(walk.workspaces()[0]), "shop@2.4.0");
}

#[test]
fn a_subproject_lockfile_is_a_project_named_after_its_directory() {
    let dir = repository(&[
        ("settings.gradle", "rootProject.name = 'shop'\n"),
        ("api/gradle.lockfile", GRADLE_LOCKFILE),
    ]);
    let walk = Walk::new(lockfiles::load(dir.path(), &["**/gradle.lockfile".into()]).unwrap());
    let projects: Vec<_> = walk
        .workspaces()
        .iter()
        .map(|&id| {
            let node = &walk.graph.nodes[id];
            format!("{} in {}", node.name, node.workspace.as_deref().unwrap())
        })
        .collect();
    assert_eq!(projects, ["api in api"]);
}

#[test]
fn maven_versions_order_by_qualifier_then_numerically() {
    use codemod_deps::semver::compare;
    let ordered = [
        "1.0-alpha-1",
        "1.0-beta-2",
        "1.0-M1",
        "1.0-RC1",
        "1.0-SNAPSHOT",
        "1.0",
        "1.0-sp1",
        "1.0.1",
        "1.9",
        "1.10",
    ];
    let mut shuffled = ordered;
    shuffled.reverse();
    shuffled.sort_by(|a, b| compare(Ecosystem::Maven, a, b));
    assert_eq!(shuffled, ordered);
}

#[test]
fn any_text_sorts_as_a_maven_version_without_breaking_the_order() {
    use codemod_deps::semver::compare;
    // Long enough that an order that is not total would make the sort panic.
    let mut versions: Vec<String> = (0..60)
        .flat_map(|n| [format!("{n}.0"), format!("{n}-x"), format!("build {n}")])
        .collect();
    versions.sort_by(|a, b| compare(Ecosystem::Maven, a, b));
    let sorted = versions
        .windows(2)
        .all(|pair| compare(Ecosystem::Maven, &pair[0], &pair[1]).is_le());
    assert!(sorted);
}

#[test]
fn trailing_zeros_and_release_qualifiers_name_the_same_maven_version() {
    use codemod_deps::semver::compare;
    assert_eq!(
        ["1.0.0", "1-ga", "1.0.Final"].map(|other| compare(Ecosystem::Maven, "1.0", other)),
        [std::cmp::Ordering::Equal; 3]
    );
}

#[test]
fn maven_ranges_have_inclusive_and_exclusive_bounds_and_unions() {
    use codemod_deps::semver::satisfies;
    let check = |range, version| satisfies(Ecosystem::Maven, range, version);
    assert_eq!(
        [
            check("[1.0,2.0)", "2.0"),
            check("(,1.0]", "1.0"),
            check("[1.5]", "1.5.0"),
            check("(,1.0],[1.2,)", "1.1"),
        ],
        [Some(false), Some(true), Some(true), Some(false)]
    );
}

#[test]
fn a_bare_maven_version_accepts_that_version_or_higher() {
    use codemod_deps::semver::satisfies;
    assert_eq!(
        ["0.9", "1.0", "2.3"].map(|version| satisfies(Ecosystem::Maven, "1.0", version)),
        [Some(false), Some(true), Some(true)]
    );
}

#[test]
fn a_gradle_prefix_version_accepts_versions_that_start_with_it() {
    use codemod_deps::semver::satisfies;
    assert_eq!(
        ["1.9.2", "2.0"].map(|version| satisfies(Ecosystem::Maven, "1.+", version)),
        [Some(true), Some(false)]
    );
}

#[test]
fn a_maven_range_that_cannot_be_read_is_none() {
    use codemod_deps::semver::satisfies;
    assert_eq!(
        ["latest.release", "[1.0", "^1.0"].map(|range| satisfies(Ecosystem::Maven, range, "1.0")),
        [None; 3]
    );
}

#[test]
fn maven_prereleases_are_alpha_beta_milestone_rc_and_snapshot_versions() {
    use codemod_deps::semver::is_prerelease;
    assert_eq!(
        [
            "1.0-M1",
            "2.0.0-SNAPSHOT",
            "5.0.0-CR2",
            "31.1-jre",
            "1.0.Final"
        ]
        .map(|version| is_prerelease(Ecosystem::Maven, version)),
        [true, true, true, false, false]
    );
}

#[test]
fn a_maven_upgrade_is_breaking_when_its_leftmost_non_zero_part_changes() {
    use codemod_deps::semver::is_breaking;
    assert_eq!(
        [
            is_breaking(Ecosystem::Maven, "31.1-jre", "32.0.0-jre"),
            is_breaking(Ecosystem::Maven, "0.9", "0.10"),
            is_breaking(Ecosystem::Maven, "5.9.0", "5.10.1"),
        ],
        [true, true, false]
    );
}

#[test]
fn maven_metadata_releases_are_the_versions_under_versioning() {
    use codemod_deps::registry::maven_releases;
    let document = r#"<?xml version="1.0" encoding="UTF-8"?>
<metadata>
  <groupId>com.google.guava</groupId>
  <artifactId>failureaccess</artifactId>
  <version>1.0</version>
  <versioning>
    <latest>1.0.2</latest>
    <release>1.0.2</release>
    <versions>
      <version>1.0.1</version>
      <version>1.0.2</version>
    </versions>
  </versioning>
</metadata>"#;
    let versions: Vec<_> = maven_releases(document)
        .unwrap()
        .into_iter()
        .map(|release| release.version)
        .collect();
    assert_eq!(versions, ["1.0.1", "1.0.2"]);
}
