use crate::auth::TokenStorage;
use anyhow::{anyhow, Result};
use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct RegistrySearchRequest {
    pub query: Option<String>,
    pub language: Option<String>,
    pub framework: Option<String>,
    pub category: Option<String>,
    pub size: u32,
    pub from: u32,
    pub scope: Option<String>,
    pub registry: Option<String>,
}

impl Default for RegistrySearchRequest {
    fn default() -> Self {
        Self {
            query: None,
            language: None,
            framework: None,
            category: None,
            size: 20,
            from: 0,
            scope: None,
            registry: None,
        }
    }
}

#[derive(Debug)]
pub struct RegistrySearchResult {
    pub registry_url: String,
    pub response: RegistrySearchResponse,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct RegistrySearchResponse {
    pub total: u32,
    pub packages: Vec<RegistrySearchPackage>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct RegistrySearchPackage {
    pub id: String,
    pub name: String,
    pub scope: Option<String>,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub author: String,
    pub license: Option<String>,
    pub repository: Option<String>,
    pub homepage: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub category: Option<String>,
    pub latest_version: Option<String>,
    #[serde(default)]
    pub download_count: u32,
    #[serde(default)]
    pub star_count: u32,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub owner: RegistryPackageOwner,
    pub organization: Option<RegistryPackageOrganization>,
    pub frameworks: Option<Vec<String>>,
    pub languages: Option<Vec<String>>,
    pub version_ranges: Option<Vec<String>>,
    pub confidence_hints: Option<Vec<String>>,
    pub known_limits: Option<Vec<String>>,
    pub quality_score: Option<f32>,
    pub maintenance_score: Option<f32>,
    pub adoption_score: Option<f32>,
    pub pro_required: Option<bool>,
    pub login_required: Option<bool>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct RegistryPackageOwner {
    pub id: String,
    pub username: String,
    pub name: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct RegistryPackageOrganization {
    pub id: String,
    pub name: String,
    pub slug: String,
}

pub const REQUIRED_SUITABILITY_FIELDS: [&str; 10] = [
    "frameworks",
    "languages",
    "version_ranges",
    "confidence_hints",
    "known_limits",
    "quality_score",
    "maintenance_score",
    "adoption_score",
    "pro_required",
    "login_required",
];

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct MetadataCoverage {
    pub required_fields: Vec<&'static str>,
    pub present_fields: Vec<&'static str>,
    pub missing_fields: Vec<&'static str>,
    pub completeness_ratio: f32,
    pub ready_for_threshold_routing: bool,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct SearchCoverageSummary {
    pub total_packages: usize,
    pub required_fields: Vec<&'static str>,
    pub packages_ready_for_threshold_routing: usize,
    pub packages_missing_contract_fields: usize,
    pub missing_field_counts: BTreeMap<&'static str, usize>,
}

pub async fn search_registry(request: RegistrySearchRequest) -> Result<RegistrySearchResult> {
    let storage = TokenStorage::new()?;
    let config = storage.load_config()?;

    let registry_url = request.registry.unwrap_or(config.default_registry);
    debug!("Searching packages in registry: {registry_url}");

    let endpoint = format!("{registry_url}/api/v1/registry/search");
    let mut query_params: Vec<(&str, String)> = Vec::new();

    if let Some(query) = request.query {
        query_params.push(("q", query));
    }
    if let Some(language) = request.language {
        query_params.push(("language", language));
    }
    if let Some(framework) = request.framework {
        query_params.push(("framework", framework));
    }
    if let Some(category) = request.category {
        query_params.push(("category", category));
    }
    if let Some(scope) = request.scope {
        query_params.push(("scope", scope));
    }
    query_params.push(("size", request.size.to_string()));
    query_params.push(("from", request.from.to_string()));

    let client = reqwest::Client::new();
    let mut reqwest_request = client.get(&endpoint).query(&query_params);

    if let Ok(Some(auth)) = storage.get_auth_for_registry(&registry_url) {
        reqwest_request = reqwest_request.header(
            "Authorization",
            format!("Bearer {}", auth.tokens.access_token),
        );
    }

    let response = reqwest_request.send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "Search failed with status {}: {}",
            status,
            error_text
        ));
    }

    let response_body: RegistrySearchResponse = response.json().await?;
    Ok(RegistrySearchResult {
        registry_url,
        response: response_body,
    })
}

pub fn summarize_search_coverage(packages: &[RegistrySearchPackage]) -> SearchCoverageSummary {
    let mut packages_ready_for_threshold_routing = 0usize;
    let mut missing_field_counts: BTreeMap<&'static str, usize> = REQUIRED_SUITABILITY_FIELDS
        .iter()
        .map(|field| (*field, 0usize))
        .collect();

    for package in packages {
        let metadata_coverage = package.metadata_coverage();
        if metadata_coverage.ready_for_threshold_routing {
            packages_ready_for_threshold_routing += 1;
        }

        for missing_field in &metadata_coverage.missing_fields {
            if let Some(count) = missing_field_counts.get_mut(missing_field) {
                *count += 1;
            }
        }
    }

    SearchCoverageSummary {
        total_packages: packages.len(),
        required_fields: REQUIRED_SUITABILITY_FIELDS.to_vec(),
        packages_ready_for_threshold_routing,
        packages_missing_contract_fields: packages.len() - packages_ready_for_threshold_routing,
        missing_field_counts,
    }
}

impl RegistrySearchPackage {
    pub fn metadata_coverage(&self) -> MetadataCoverage {
        let field_presence = [
            ("frameworks", is_non_empty_list(self.frameworks.as_ref())),
            ("languages", is_non_empty_list(self.languages.as_ref())),
            (
                "version_ranges",
                is_non_empty_list(self.version_ranges.as_ref()),
            ),
            (
                "confidence_hints",
                is_non_empty_list(self.confidence_hints.as_ref()),
            ),
            (
                "known_limits",
                is_non_empty_list(self.known_limits.as_ref()),
            ),
            ("quality_score", self.quality_score.is_some()),
            ("maintenance_score", self.maintenance_score.is_some()),
            ("adoption_score", self.adoption_score.is_some()),
            ("pro_required", self.pro_required.is_some()),
            ("login_required", self.login_required.is_some()),
        ];

        let present_fields = field_presence
            .iter()
            .filter_map(|(field, present)| present.then_some(*field))
            .collect::<Vec<_>>();
        let missing_fields = field_presence
            .iter()
            .filter_map(|(field, present)| (!present).then_some(*field))
            .collect::<Vec<_>>();

        MetadataCoverage {
            required_fields: REQUIRED_SUITABILITY_FIELDS.to_vec(),
            present_fields: present_fields.clone(),
            missing_fields: missing_fields.clone(),
            completeness_ratio: present_fields.len() as f32
                / REQUIRED_SUITABILITY_FIELDS.len() as f32,
            ready_for_threshold_routing: missing_fields.is_empty(),
        }
    }
}

fn is_non_empty_list(value: Option<&Vec<String>>) -> bool {
    value.map(|items| !items.is_empty()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_package() -> RegistrySearchPackage {
        RegistrySearchPackage {
            id: "pkg-123".to_string(),
            name: "jest-to-vitest".to_string(),
            scope: Some("codemod".to_string()),
            display_name: Some("Jest to Vitest".to_string()),
            description: Some("Migrate test runner".to_string()),
            author: "codemod".to_string(),
            license: Some("MIT".to_string()),
            repository: Some("https://github.com/codemod".to_string()),
            homepage: Some("https://codemod.com".to_string()),
            keywords: vec!["jest".to_string(), "vitest".to_string()],
            category: Some("testing".to_string()),
            latest_version: Some("1.0.0".to_string()),
            download_count: 100,
            star_count: 10,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: Some("2026-01-02T00:00:00Z".to_string()),
            owner: RegistryPackageOwner {
                id: "owner-1".to_string(),
                username: "codemod".to_string(),
                name: "Codemod".to_string(),
            },
            organization: Some(RegistryPackageOrganization {
                id: "org-1".to_string(),
                name: "Codemod".to_string(),
                slug: "codemod".to_string(),
            }),
            frameworks: None,
            languages: None,
            version_ranges: None,
            confidence_hints: None,
            known_limits: None,
            quality_score: None,
            maintenance_score: None,
            adoption_score: None,
            pro_required: None,
            login_required: None,
        }
    }

    #[test]
    fn metadata_coverage_marks_baseline_payload_as_missing_contract_fields() {
        let package = sample_package();
        let coverage = package.metadata_coverage();

        assert_eq!(coverage.present_fields.len(), 0);
        assert_eq!(
            coverage.missing_fields.len(),
            REQUIRED_SUITABILITY_FIELDS.len()
        );
        assert_eq!(coverage.completeness_ratio, 0.0);
        assert!(!coverage.ready_for_threshold_routing);
    }

    #[test]
    fn metadata_coverage_marks_package_ready_when_all_required_fields_exist() {
        let mut package = sample_package();
        package.frameworks = Some(vec!["react".to_string()]);
        package.languages = Some(vec!["typescript".to_string()]);
        package.version_ranges = Some(vec!["react=>=18".to_string()]);
        package.confidence_hints = Some(vec!["targets declared".to_string()]);
        package.known_limits = Some(vec!["jsx-runtime edge cases".to_string()]);
        package.quality_score = Some(0.92);
        package.maintenance_score = Some(0.87);
        package.adoption_score = Some(0.76);
        package.pro_required = Some(false);
        package.login_required = Some(true);

        let coverage = package.metadata_coverage();

        assert_eq!(coverage.missing_fields.len(), 0);
        assert_eq!(
            coverage.present_fields.len(),
            REQUIRED_SUITABILITY_FIELDS.len()
        );
        assert_eq!(coverage.completeness_ratio, 1.0);
        assert!(coverage.ready_for_threshold_routing);
    }

    #[test]
    fn summarize_search_coverage_counts_missing_fields() {
        let mut ready_package = sample_package();
        ready_package.frameworks = Some(vec!["react".to_string()]);
        ready_package.languages = Some(vec!["typescript".to_string()]);
        ready_package.version_ranges = Some(vec!["react=>=18".to_string()]);
        ready_package.confidence_hints = Some(vec!["targets declared".to_string()]);
        ready_package.known_limits = Some(vec!["none".to_string()]);
        ready_package.quality_score = Some(0.9);
        ready_package.maintenance_score = Some(0.8);
        ready_package.adoption_score = Some(0.7);
        ready_package.pro_required = Some(false);
        ready_package.login_required = Some(false);

        let missing_package = sample_package();
        let summary = summarize_search_coverage(&[ready_package, missing_package]);

        assert_eq!(summary.total_packages, 2);
        assert_eq!(summary.packages_ready_for_threshold_routing, 1);
        assert_eq!(summary.packages_missing_contract_fields, 1);
        assert_eq!(summary.missing_field_counts["frameworks"], 1);
        assert_eq!(summary.missing_field_counts["login_required"], 1);
    }
}
