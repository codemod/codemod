use serde::{Deserialize, Serialize};

pub(crate) const PLACEHOLDER_AUTHOR: &str = "Author <author@example.com>";
pub(crate) const PLACEHOLDER_AUTHOR_EMAIL: &str = "author@example.com";

pub(crate) fn normalize_author(author: Option<String>) -> Option<String> {
    author
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn is_placeholder_author(author: Option<&str>) -> bool {
    author.is_some_and(|value| value.contains(PLACEHOLDER_AUTHOR_EMAIL))
}

#[derive(Deserialize, Serialize, Debug)]
pub(crate) struct CodemodManifest {
    pub(crate) schema_version: String,
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) copyright: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bugs: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) registry: Option<RegistryConfig>,
    pub(crate) workflow: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) targets: Option<TargetConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) dependencies: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) keywords: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) readme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) changelog: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) documentation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) validation: Option<ValidationConfig>,
    pub(crate) capabilities: Option<Vec<String>>,
}

#[derive(Deserialize, Serialize, Debug)]
pub(crate) struct RegistryConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) access: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) visibility: Option<String>,
}

#[derive(Deserialize, Serialize, Debug)]
pub(crate) struct TargetConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    languages: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frameworks: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    versions: Option<std::collections::HashMap<String, String>>,
}

#[derive(Deserialize, Serialize, Debug)]
pub(crate) struct ValidationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    strict: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    require_tests: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_test_coverage: Option<u32>,
}
