use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use butterflow_models::{Error, Result};
use dependency_files::{detect_context_file, detect_lock_file, Ecosystem};
use ignore::WalkBuilder;
use regex::Regex;
use serde::{Deserialize, Serialize};

const FACTS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowFacts {
    pub schema_version: u32,
    pub ecosystems: Vec<EcosystemFact>,
    pub dependencies: Vec<DependencyFact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EcosystemFact {
    pub ecosystem: Ecosystem,
    pub source: EcosystemFactSource,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EcosystemFactSource {
    ContextFile,
    LockFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyFact {
    pub ecosystem: Ecosystem,
    pub name: String,
    pub version: String,
    pub path: String,
    pub dependency_type: Option<String>,
}

impl WorkflowFacts {
    pub fn empty() -> Self {
        Self {
            schema_version: FACTS_SCHEMA_VERSION,
            ecosystems: Vec::new(),
            dependencies: Vec::new(),
        }
    }

    pub fn collect_from_path(target_path: &Path) -> Result<Self> {
        if !target_path.exists() {
            return Err(Error::Other(format!(
                "target path does not exist: {}",
                target_path.display()
            )));
        }

        let mut facts = Self::empty();
        let mut ecosystem_keys = BTreeSet::new();
        let mut dependency_keys = BTreeSet::new();

        for result in WalkBuilder::new(target_path)
            .standard_filters(true)
            .hidden(false)
            .build()
        {
            let entry = match result {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let file_type = match entry.file_type() {
                Some(file_type) if file_type.is_file() => file_type,
                _ => continue,
            };
            if !file_type.is_file() {
                continue;
            }

            let rel_path = relative_path(target_path, entry.path());
            if let Some(ecosystem) = detect_context_file(&rel_path) {
                push_ecosystem_fact(
                    &mut facts,
                    &mut ecosystem_keys,
                    ecosystem,
                    EcosystemFactSource::ContextFile,
                    rel_path.clone(),
                );
                parse_dependency_facts(
                    entry.path(),
                    &rel_path,
                    ecosystem,
                    &mut facts,
                    &mut dependency_keys,
                );
            } else if let Some(ecosystem) = detect_lock_file(&rel_path) {
                push_ecosystem_fact(
                    &mut facts,
                    &mut ecosystem_keys,
                    ecosystem,
                    EcosystemFactSource::LockFile,
                    rel_path.clone(),
                );
                parse_dependency_facts(
                    entry.path(),
                    &rel_path,
                    ecosystem,
                    &mut facts,
                    &mut dependency_keys,
                );
            }
        }

        Ok(facts)
    }

    pub fn has_ecosystem(&self, ecosystem: Ecosystem) -> bool {
        self.ecosystems
            .iter()
            .any(|fact| fact.ecosystem == ecosystem)
    }
}

fn push_ecosystem_fact(
    facts: &mut WorkflowFacts,
    keys: &mut BTreeSet<(Ecosystem, EcosystemFactSource, String)>,
    ecosystem: Ecosystem,
    source: EcosystemFactSource,
    path: String,
) {
    if keys.insert((ecosystem, source, path.clone())) {
        facts.ecosystems.push(EcosystemFact {
            ecosystem,
            source,
            path,
        });
    }
}

fn parse_dependency_facts(
    path: &Path,
    rel_path: &str,
    ecosystem: Ecosystem,
    facts: &mut WorkflowFacts,
    keys: &mut BTreeSet<(Ecosystem, String, String, String, Option<String>)>,
) {
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };

    let dependencies = match dependency_files::file_name(rel_path) {
        "package.json" => parse_package_json_dependencies(&content, rel_path),
        "Cargo.toml" => parse_cargo_toml_dependencies(&content, rel_path),
        "pyproject.toml" => parse_pyproject_dependencies(&content, rel_path),
        "go.mod" => parse_go_mod_dependencies(&content, rel_path),
        "requirements.txt" => parse_requirements_dependencies(&content, rel_path),
        "pom.xml" => parse_maven_dependencies(&content, rel_path),
        "build.gradle" | "build.gradle.kts" => parse_gradle_dependencies(&content, rel_path),
        _ => Vec::new(),
    };

    for dependency in dependencies {
        if dependency.ecosystem != ecosystem {
            continue;
        }
        let key = (
            dependency.ecosystem,
            dependency.name.clone(),
            dependency.version.clone(),
            dependency.path.clone(),
            dependency.dependency_type.clone(),
        );
        if keys.insert(key) {
            facts.dependencies.push(dependency);
        }
    }
}

fn parse_package_json_dependencies(content: &str, path: &str) -> Vec<DependencyFact> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return Vec::new();
    };
    let mut facts = Vec::new();
    for section in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        let Some(deps) = value.get(section).and_then(|value| value.as_object()) else {
            continue;
        };
        for (name, version) in deps {
            if let Some(version) = version.as_str() {
                push_dependency(
                    &mut facts,
                    Ecosystem::Npm,
                    name,
                    version,
                    path,
                    Some(section),
                );
            }
        }
    }
    facts
}

fn parse_cargo_toml_dependencies(content: &str, path: &str) -> Vec<DependencyFact> {
    let Ok(value) = content.parse::<toml::Value>() else {
        return Vec::new();
    };
    let mut facts = Vec::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        let Some(deps) = value.get(section).and_then(|value| value.as_table()) else {
            continue;
        };
        for (name, value) in deps {
            if let Some(version) = cargo_dependency_version(value) {
                push_dependency(
                    &mut facts,
                    Ecosystem::Cargo,
                    name,
                    version,
                    path,
                    Some(section),
                );
            }
        }
    }
    facts
}

fn cargo_dependency_version(value: &toml::Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.as_table()?.get("version")?.as_str())
}

fn parse_pyproject_dependencies(content: &str, path: &str) -> Vec<DependencyFact> {
    let Ok(value) = content.parse::<toml::Value>() else {
        return Vec::new();
    };
    let mut facts = Vec::new();

    if let Some(dependencies) = value
        .get("project")
        .and_then(|project| project.get("dependencies"))
        .and_then(|dependencies| dependencies.as_array())
    {
        for dependency in dependencies {
            if let Some(spec) = dependency.as_str() {
                if let (Some(name), Some(version)) = (
                    python_dependency_name(spec),
                    python_dependency_version_requirement(spec),
                ) {
                    push_dependency(
                        &mut facts,
                        Ecosystem::PyPI,
                        name,
                        &version,
                        path,
                        Some("dependencies"),
                    );
                }
            }
        }
    }

    if let Some(dependencies) = value
        .get("tool")
        .and_then(|tool| tool.get("poetry"))
        .and_then(|poetry| poetry.get("dependencies"))
        .and_then(|dependencies| dependencies.as_table())
    {
        for (name, dependency) in dependencies {
            if name == "python" {
                continue;
            }
            if let Some(version) = dependency.as_str().or_else(|| {
                dependency
                    .as_table()
                    .and_then(|table| table.get("version"))
                    .and_then(|version| version.as_str())
            }) {
                push_dependency(
                    &mut facts,
                    Ecosystem::PyPI,
                    name,
                    version,
                    path,
                    Some("tool.poetry.dependencies"),
                );
            }
        }
    }

    facts
}

fn parse_go_mod_dependencies(content: &str, path: &str) -> Vec<DependencyFact> {
    let mut facts = Vec::new();
    let mut in_require_block = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "require (" {
            in_require_block = true;
            continue;
        }
        if in_require_block && trimmed == ")" {
            in_require_block = false;
            continue;
        }

        let require_line = if in_require_block {
            trimmed
        } else if let Some(rest) = trimmed.strip_prefix("require ") {
            rest.trim()
        } else {
            continue;
        };

        let require_line = require_line.split("//").next().unwrap_or("").trim();
        let mut parts = require_line.split_whitespace();
        let (Some(name), Some(version)) = (parts.next(), parts.next()) else {
            continue;
        };
        push_dependency(
            &mut facts,
            Ecosystem::Go,
            name,
            version,
            path,
            Some("require"),
        );
    }

    facts
}

fn parse_requirements_dependencies(content: &str, path: &str) -> Vec<DependencyFact> {
    let mut facts = Vec::new();
    for line in content.lines() {
        let trimmed = line.split('#').next().unwrap_or("").trim();
        if trimmed.is_empty() || trimmed.starts_with('-') {
            continue;
        }
        if let (Some(name), Some(version)) = (
            python_dependency_name(trimmed),
            python_dependency_version_requirement(trimmed),
        ) {
            push_dependency(
                &mut facts,
                Ecosystem::PyPI,
                name,
                &version,
                path,
                Some("requirements"),
            );
        }
    }
    facts
}

fn parse_maven_dependencies(content: &str, path: &str) -> Vec<DependencyFact> {
    let dependency_regex = Regex::new(r"(?s)<dependency\b[^>]*>(.*?)</dependency>").unwrap();
    let content = remove_maven_non_project_dependency_sections(content);
    let mut facts = Vec::new();

    for dependency in dependency_regex.captures_iter(&content) {
        let Some(block) = dependency.get(1).map(|match_| match_.as_str()) else {
            continue;
        };
        let (Some(group_id), Some(artifact_id), Some(version)) = (
            xml_tag_value(block, "groupId"),
            xml_tag_value(block, "artifactId"),
            xml_tag_value(block, "version"),
        ) else {
            continue;
        };
        if version.contains("${") {
            continue;
        }
        let name = format!("{group_id}:{artifact_id}");
        push_dependency(
            &mut facts,
            Ecosystem::Java,
            &name,
            version,
            path,
            Some("dependencies"),
        );
    }

    facts
}

fn remove_maven_non_project_dependency_sections(content: &str) -> String {
    Regex::new(
        r"(?s)<dependencyManagement\b[^>]*>.*?</dependencyManagement>|<build\b[^>]*>.*?</build>",
    )
    .unwrap()
    .replace_all(content, "")
    .into_owned()
}

fn xml_tag_value<'a>(content: &'a str, tag: &str) -> Option<&'a str> {
    let tag_regex = Regex::new(&format!(r"(?s)<{tag}\b[^>]*>\s*([^<]+?)\s*</{tag}>")).unwrap();
    tag_regex
        .captures(content)?
        .get(1)
        .map(|match_| match_.as_str().trim())
}

fn parse_gradle_dependencies(content: &str, path: &str) -> Vec<DependencyFact> {
    let dependency_regex =
        Regex::new(r#"(?m)^\s*([A-Za-z][A-Za-z0-9_]*)\s*(?:\(\s*)?["']([^"']+)["']"#).unwrap();
    let mut facts = Vec::new();

    for dependency in dependency_regex.captures_iter(content) {
        let (Some(configuration), Some(spec)) = (
            dependency.get(1).map(|match_| match_.as_str()),
            dependency.get(2).map(|match_| match_.as_str()),
        ) else {
            continue;
        };
        if !is_gradle_dependency_configuration(configuration) {
            continue;
        }
        let parts = spec.split(':').collect::<Vec<_>>();
        let [group, artifact, version] = parts.as_slice() else {
            continue;
        };
        if version.contains('$') {
            continue;
        }
        let name = format!("{group}:{artifact}");
        push_dependency(
            &mut facts,
            Ecosystem::Java,
            &name,
            version,
            path,
            Some(configuration),
        );
    }

    facts
}

fn is_gradle_dependency_configuration(configuration: &str) -> bool {
    matches!(
        configuration,
        "api"
            | "implementation"
            | "compileOnly"
            | "compileOnlyApi"
            | "runtimeOnly"
            | "testImplementation"
            | "testCompileOnly"
            | "testRuntimeOnly"
            | "annotationProcessor"
            | "testAnnotationProcessor"
    )
}

fn python_dependency_name(spec: &str) -> Option<&str> {
    let name = spec
        .split(['<', '>', '=', '!', '~', ';', '[', ' '])
        .next()
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn python_dependency_version_requirement(spec: &str) -> Option<String> {
    let spec = spec.split(';').next().unwrap_or("").trim();
    let name = python_dependency_name(spec)?;
    let mut rest = spec.get(name.len()..)?.trim_start();

    if let Some(after_extras) = rest.strip_prefix('[') {
        let extras_end = after_extras.find(']')?;
        rest = after_extras.get(extras_end + 1..)?.trim_start();
    }

    if rest.is_empty() {
        return None;
    }

    let normalized = rest
        .split(',')
        .filter_map(|part| normalize_python_version_comparator(part.trim()))
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn normalize_python_version_comparator(comparator: &str) -> Option<String> {
    for operator in ["==", "!=", ">=", "<=", "~=", ">", "<", "="] {
        if let Some(version) = comparator.strip_prefix(operator) {
            return Some(format!("{operator}{}", version.trim_start()));
        }
    }
    None
}

fn push_dependency(
    facts: &mut Vec<DependencyFact>,
    ecosystem: Ecosystem,
    name: &str,
    version: &str,
    path: &str,
    dependency_type: Option<&str>,
) {
    let name = name.trim();
    let version = version.trim();
    if name.is_empty() || version.is_empty() {
        return;
    }
    facts.push(DependencyFact {
        ecosystem,
        name: name.to_string(),
        version: version.to_string(),
        path: path.to_string(),
        dependency_type: dependency_type.map(ToString::to_string),
    });
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_ecosystem_and_dependency_facts() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("package.json"),
            r#"{"dependencies":{"react":"^18.2.0"},"devDependencies":{"vite":"^5.0.0"}}"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("Cargo.toml"),
            r#"[dependencies]
anyhow = "1"
serde = { version = "1", features = ["derive"] }
"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("pyproject.toml"),
            r#"[project]
dependencies = ["requests>=2.31.0"]
"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("go.mod"),
            r#"module example.com/app

require (
    github.com/gin-gonic/gin v1.9.1
)
"#,
        )
        .unwrap();
        fs::write(temp.path().join("pom.xml"), "<project></project>").unwrap();
        fs::write(
            temp.path().join("build.gradle"),
            r#"dependencies {
    implementation "org.slf4j:slf4j-api:2.0.9"
}
"#,
        )
        .unwrap();

        let facts = WorkflowFacts::collect_from_path(temp.path()).unwrap();

        assert!(facts.has_ecosystem(Ecosystem::Npm));
        assert!(facts.has_ecosystem(Ecosystem::Cargo));
        assert!(facts.has_ecosystem(Ecosystem::PyPI));
        assert!(facts.has_ecosystem(Ecosystem::Go));
        assert!(facts.has_ecosystem(Ecosystem::Java));
        assert_dependency(&facts, Ecosystem::Npm, "react", "^18.2.0");
        assert_dependency(&facts, Ecosystem::Npm, "vite", "^5.0.0");
        assert_dependency(&facts, Ecosystem::Cargo, "anyhow", "1");
        assert_dependency(&facts, Ecosystem::Cargo, "serde", "1");
        assert_dependency(&facts, Ecosystem::PyPI, "requests", ">=2.31.0");
        assert_dependency(&facts, Ecosystem::Go, "github.com/gin-gonic/gin", "v1.9.1");
        assert_dependency(&facts, Ecosystem::Java, "org.slf4j:slf4j-api", "2.0.9");
    }

    #[test]
    fn parses_python_requirement_versions_without_package_name() {
        let facts = parse_requirements_dependencies(
            r#"
requests==2.31.0
urllib3 >= 2.0.0, <3.0.0
pkg-extra[security]~=1.2
"#,
            "requirements.txt",
        );

        assert_dependency_name_version(&facts, "requests", "==2.31.0");
        assert_dependency_name_version(&facts, "urllib3", ">=2.0.0 <3.0.0");
        assert_dependency_name_version(&facts, "pkg-extra", "~=1.2");
    }

    #[test]
    fn parses_pyproject_dependency_versions_without_package_name() {
        let facts = parse_pyproject_dependencies(
            r#"[project]
dependencies = ["requests>=2.31.0", "urllib3 < 3.0.0"]
"#,
            "pyproject.toml",
        );

        assert_dependency_name_version(&facts, "requests", ">=2.31.0");
        assert_dependency_name_version(&facts, "urllib3", "<3.0.0");
    }

    #[test]
    fn parses_direct_maven_dependencies() {
        let facts = parse_maven_dependencies(
            r#"
<project>
  <dependencies>
    <dependency>
      <groupId>org.junit.jupiter</groupId>
      <artifactId>junit-jupiter-api</artifactId>
      <version>5.10.2</version>
    </dependency>
    <dependency>
      <groupId>org.example</groupId>
      <artifactId>managed</artifactId>
      <version>${managed.version}</version>
    </dependency>
  </dependencies>
  <dependencyManagement>
    <dependencies>
      <dependency>
        <groupId>org.example</groupId>
        <artifactId>managed-direct</artifactId>
        <version>1.0.0</version>
      </dependency>
    </dependencies>
  </dependencyManagement>
  <build>
    <plugins>
      <plugin>
        <dependencies>
          <dependency>
            <groupId>org.example</groupId>
            <artifactId>plugin-dependency</artifactId>
            <version>1.0.0</version>
          </dependency>
        </dependencies>
      </plugin>
    </plugins>
  </build>
</project>
"#,
            "pom.xml",
        );

        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].name, "org.junit.jupiter:junit-jupiter-api");
        assert_eq!(facts[0].version, "5.10.2");
    }

    #[test]
    fn parses_direct_gradle_dependencies() {
        let facts = parse_gradle_dependencies(
            r#"
dependencies {
    implementation("org.slf4j:slf4j-api:2.0.9")
    testImplementation 'org.junit.jupiter:junit-jupiter-api:5.10.2'
    classpath "org.example:build-plugin:1.0.0"
    implementation(libs.slf4j)
}
"#,
            "build.gradle.kts",
        );

        assert_eq!(facts.len(), 2);
        assert_dependency_name_version(&facts, "org.slf4j:slf4j-api", "2.0.9");
        assert_dependency_name_version(&facts, "org.junit.jupiter:junit-jupiter-api", "5.10.2");
    }

    #[test]
    fn records_lock_file_ecosystem_without_parsing_lock_contents() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("package-lock.json"), "{}").unwrap();

        let facts = WorkflowFacts::collect_from_path(temp.path()).unwrap();

        assert_eq!(facts.dependencies, Vec::new());
        assert!(facts.ecosystems.iter().any(|fact| {
            fact.ecosystem == Ecosystem::Npm
                && fact.source == EcosystemFactSource::LockFile
                && fact.path == "package-lock.json"
        }));
    }

    fn assert_dependency(facts: &WorkflowFacts, ecosystem: Ecosystem, name: &str, version: &str) {
        assert!(facts.dependencies.iter().any(|dependency| {
            dependency.ecosystem == ecosystem
                && dependency.name == name
                && dependency.version == version
        }));
    }

    fn assert_dependency_name_version(facts: &[DependencyFact], name: &str, version: &str) {
        assert!(facts
            .iter()
            .any(|dependency| { dependency.name == name && dependency.version == version }));
    }
}
