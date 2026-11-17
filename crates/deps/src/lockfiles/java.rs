//! Gradle's `gradle.lockfile` (Gradle 6.8+, one file per project). Each
//! line is `group:artifact:version=configuration,...`; `empty=` lists the
//! configurations that resolved nothing.
//!
//! The lockfile keeps resolved versions only: no declared ranges and no
//! edges between modules. So the graph is flat: every locked module is an
//! edge from the project, with no range, and nothing chains deeper.
//!
//! A module's kind comes from its configurations: a compile or runtime
//! classpath is `prod`, or `dev` when it is a test one (`test...` or
//! `...Test...`); anything else (annotation processors, kapt and ksp,
//! Kotlin compiler plugins, code-quality tools) is `build`. A module in
//! several configurations is one edge, `prod` over `dev` over `build`.

use std::path::Path;

use super::{ReadFile, UNVERSIONED};
use crate::model::{Ecosystem, Graph, Kind};

/// `dir` is the project's directory, whose name is the project's name when
/// no `settings.gradle(.kts)` beside the lockfile sets `rootProject.name`.
pub fn gradle(text: &str, read: ReadFile, dir: &Path) -> Result<Graph, String> {
    let mut graph = Graph::from_lockfile("gradle.lockfile");
    let name = ["settings.gradle.kts", "settings.gradle"]
        .iter()
        .find_map(|file| root_project_name(&read(file)?))
        .unwrap_or_else(|| directory_name(dir));
    let version = read("gradle.properties").and_then(|text| property(&text, "version"));
    let root = graph.workspace(
        Ecosystem::Maven,
        &name,
        version.as_deref().unwrap_or(UNVERSIONED),
        ".",
    );

    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("empty=") {
            continue;
        }
        let (group, artifact, version, configurations) = line
            .split_once('=')
            .and_then(|(module, configurations)| {
                let mut parts = module.split(':');
                let found = (parts.next()?, parts.next()?, parts.next()?, configurations);
                parts.next().is_none().then_some(found)
            })
            .ok_or_else(|| {
                format!(
                    "line {} is not group:artifact:version=configurations",
                    number + 1
                )
            })?;
        let kind = configurations
            .split(',')
            .map(|configuration| configuration_kind(configuration.trim()))
            .min_by_key(|&kind| {
                [Kind::Prod, Kind::Dev, Kind::Build]
                    .iter()
                    .position(|&k| k == kind)
            })
            .unwrap_or(Kind::Build);
        let name = format!("{group}:{artifact}");
        let to = graph.node(Ecosystem::Maven, &name, version);
        graph.edge(root, Some(to), &name, None, kind);
    }
    Ok(graph)
}

fn configuration_kind(configuration: &str) -> Kind {
    let lower = configuration.to_ascii_lowercase();
    if !(lower.ends_with("compileclasspath") || lower.ends_with("runtimeclasspath")) {
        Kind::Build
    } else if configuration.starts_with("test") || configuration.contains("Test") {
        Kind::Dev
    } else {
        Kind::Prod
    }
}

/// `rootProject.name = "app"` (Kotlin) or `rootProject.name = 'app'` (Groovy).
fn root_project_name(settings: &str) -> Option<String> {
    settings.lines().find_map(|line| {
        let value = line
            .trim()
            .strip_prefix("rootProject.name")?
            .trim_start()
            .strip_prefix('=')?
            .trim_start();
        let quote = value.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        let (name, _) = value[1..].split_once(quote)?;
        Some(name.to_string())
    })
}

/// A `key=value` line of a `.properties` file.
fn property(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (name, value) = line.split_once(['=', ':'])?;
        (name.trim() == key).then(|| value.trim().to_string())
    })
}

/// Gradle's default project name: the directory's.
fn directory_name(dir: &Path) -> String {
    dir.canonicalize()
        .ok()
        .as_deref()
        .unwrap_or(dir)
        .file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| "<.>".to_string(), str::to_string)
}
