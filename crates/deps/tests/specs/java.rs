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
