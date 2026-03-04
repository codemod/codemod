use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

const MODEL_CATALOG_JSON: &str = include_str!("../data/openrouter_models.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    Exact,
    Alias,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModelLimits {
    pub context_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub match_kind: MatchKind,
    pub matched_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct SnapshotModel {
    id: String,
    canonical_slug: Option<String>,
    context_length: Option<u64>,
    max_output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct Snapshot {
    models: Vec<SnapshotModel>,
}

#[derive(Debug, Clone)]
struct IndexedModel {
    id: String,
    provider: Option<String>,
    canonical_slug: Option<String>,
    alias_keys: Vec<String>,
    context_tokens: Option<u64>,
    max_output_tokens: Option<u64>,
}

#[derive(Debug, Default)]
struct CatalogIndex {
    models: Vec<IndexedModel>,
    by_id: HashMap<String, usize>,
    by_canonical_slug: HashMap<String, usize>,
    by_alias: HashMap<String, Vec<usize>>,
    by_fuzzy_alias: HashMap<String, Vec<usize>>,
}

impl CatalogIndex {
    fn from_snapshot(snapshot: Snapshot) -> Self {
        let mut index = Self::default();

        for snapshot_model in snapshot.models {
            let normalized_id = normalize_exact_key(&snapshot_model.id);
            if normalized_id.is_empty() {
                continue;
            }

            let provider = snapshot_model
                .id
                .split_once('/')
                .map(|(provider, _)| provider.to_ascii_lowercase());
            let canonical_slug = snapshot_model
                .canonical_slug
                .as_deref()
                .map(normalize_exact_key)
                .filter(|slug| !slug.is_empty());
            let mut alias_keys = Vec::new();

            let id_alias = normalize_alias_key(&snapshot_model.id);
            if !id_alias.is_empty() {
                alias_keys.push(id_alias);
            }
            if let Some(slug) = snapshot_model.canonical_slug.as_deref() {
                let slug_alias = normalize_alias_key(slug);
                if !slug_alias.is_empty() && !alias_keys.contains(&slug_alias) {
                    alias_keys.push(slug_alias);
                }
            }

            let model = IndexedModel {
                id: snapshot_model.id,
                provider,
                canonical_slug,
                alias_keys,
                context_tokens: snapshot_model.context_length,
                max_output_tokens: snapshot_model.max_output_tokens,
            };

            let current_index = index.models.len();
            index.by_id.insert(normalized_id, current_index);
            if let Some(slug) = model.canonical_slug.as_ref() {
                index.by_canonical_slug.insert(slug.clone(), current_index);
            }
            for alias in &model.alias_keys {
                index
                    .by_alias
                    .entry(alias.clone())
                    .or_default()
                    .push(current_index);

                let fuzzy_alias = fuzzy_alias_key(alias);
                if !fuzzy_alias.is_empty() {
                    index
                        .by_fuzzy_alias
                        .entry(fuzzy_alias)
                        .or_default()
                        .push(current_index);
                }
            }
            index.models.push(model);
        }

        index
    }

    fn from_json(raw: &str) -> Result<Self, serde_json::Error> {
        let snapshot: Snapshot = serde_json::from_str(raw)?;
        Ok(Self::from_snapshot(snapshot))
    }
}

static CATALOG: OnceLock<CatalogIndex> = OnceLock::new();

fn catalog() -> &'static CatalogIndex {
    CATALOG.get_or_init(|| match CatalogIndex::from_json(MODEL_CATALOG_JSON) {
        Ok(index) => index,
        Err(error) => {
            tracing::warn!(
                "Failed to parse embedded OpenRouter model catalog: {}",
                error
            );
            CatalogIndex::default()
        }
    })
}

fn normalize_exact_key(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
}

fn normalize_alias_key(raw: &str) -> String {
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return String::new();
    }

    let stripped = normalized
        .split_once('/')
        .map(|(_, model)| model)
        .unwrap_or(normalized.as_str());

    let mut collapsed = String::with_capacity(stripped.len());
    let mut previous_dash = false;
    for ch in stripped.chars() {
        if ch.is_ascii_alphanumeric() {
            collapsed.push(ch);
            previous_dash = false;
            continue;
        }

        if matches!(ch, '-' | '_' | '.' | ' ') {
            if !collapsed.is_empty() && !previous_dash {
                collapsed.push('-');
                previous_dash = true;
            }
        }
    }

    collapsed.trim_matches('-').to_string()
}

fn fuzzy_alias_key(alias: &str) -> String {
    let mut tokens = alias
        .split('-')
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return String::new();
    }

    tokens.sort_unstable();
    tokens.join("-")
}

fn protocol_provider_hint(protocol: &str) -> Option<&'static str> {
    match protocol.trim().to_ascii_lowercase().as_str() {
        "openai" | "azure_openai" => Some("openai"),
        "anthropic" => Some("anthropic"),
        "google_ai" => Some("google"),
        _ => None,
    }
}

fn resolve_with_index(
    index: &CatalogIndex,
    protocol: &str,
    model_name: &str,
) -> ResolvedModelLimits {
    let model_name_key = normalize_exact_key(model_name);
    if model_name_key.is_empty() {
        return ResolvedModelLimits {
            context_tokens: None,
            max_output_tokens: None,
            match_kind: MatchKind::None,
            matched_id: None,
        };
    }

    if let Some(model_index) = index.by_id.get(&model_name_key) {
        let model = &index.models[*model_index];
        return ResolvedModelLimits {
            context_tokens: model.context_tokens,
            max_output_tokens: model.max_output_tokens,
            match_kind: MatchKind::Exact,
            matched_id: Some(model.id.clone()),
        };
    }

    if let Some(model_index) = index.by_canonical_slug.get(&model_name_key) {
        let model = &index.models[*model_index];
        return ResolvedModelLimits {
            context_tokens: model.context_tokens,
            max_output_tokens: model.max_output_tokens,
            match_kind: MatchKind::Exact,
            matched_id: Some(model.id.clone()),
        };
    }

    let alias_key = normalize_alias_key(model_name);
    if !alias_key.is_empty() {
        if let Some(resolved) = resolve_alias_candidates(
            index,
            protocol_provider_hint(protocol),
            &index.by_alias,
            &alias_key,
        ) {
            return resolved;
        }

        let fuzzy_key = fuzzy_alias_key(&alias_key);
        if !fuzzy_key.is_empty() {
            if let Some(resolved) = resolve_alias_candidates(
                index,
                protocol_provider_hint(protocol),
                &index.by_fuzzy_alias,
                &fuzzy_key,
            ) {
                return resolved;
            }
        }
    }

    ResolvedModelLimits {
        context_tokens: None,
        max_output_tokens: None,
        match_kind: MatchKind::None,
        matched_id: None,
    }
}

fn resolve_alias_candidates(
    index: &CatalogIndex,
    provider_hint: Option<&str>,
    source: &HashMap<String, Vec<usize>>,
    key: &str,
) -> Option<ResolvedModelLimits> {
    let alias_candidates = source.get(key)?;

    if let Some(provider_hint) = provider_hint {
        let provider_matches = alias_candidates
            .iter()
            .copied()
            .filter(|candidate| {
                index.models[*candidate]
                    .provider
                    .as_deref()
                    .map(|provider| provider == provider_hint)
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();

        if provider_matches.len() == 1 {
            let model = &index.models[provider_matches[0]];
            return Some(ResolvedModelLimits {
                context_tokens: model.context_tokens,
                max_output_tokens: model.max_output_tokens,
                match_kind: MatchKind::Alias,
                matched_id: Some(model.id.clone()),
            });
        }

        if provider_matches.len() > 1 {
            return Some(ResolvedModelLimits {
                context_tokens: None,
                max_output_tokens: None,
                match_kind: MatchKind::None,
                matched_id: None,
            });
        }
    }

    if alias_candidates.len() == 1 {
        let model = &index.models[alias_candidates[0]];
        return Some(ResolvedModelLimits {
            context_tokens: model.context_tokens,
            max_output_tokens: model.max_output_tokens,
            match_kind: MatchKind::Alias,
            matched_id: Some(model.id.clone()),
        });
    }

    Some(ResolvedModelLimits {
        context_tokens: None,
        max_output_tokens: None,
        match_kind: MatchKind::None,
        matched_id: None,
    })
}

pub fn resolve_model_limits(protocol: &str, model_name: &str) -> ResolvedModelLimits {
    resolve_with_index(catalog(), protocol, model_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_index() -> CatalogIndex {
        CatalogIndex::from_snapshot(Snapshot {
            models: vec![
                SnapshotModel {
                    id: "anthropic/claude-sonnet-4-5".to_string(),
                    canonical_slug: Some("anthropic/claude-sonnet-4-5-20250929".to_string()),
                    context_length: Some(200_000),
                    max_output_tokens: Some(64_000),
                },
                SnapshotModel {
                    id: "openai/gpt-4o".to_string(),
                    canonical_slug: None,
                    context_length: Some(128_000),
                    max_output_tokens: Some(16_384),
                },
                SnapshotModel {
                    id: "google/gemini-2.5-pro".to_string(),
                    canonical_slug: None,
                    context_length: Some(1_000_000),
                    max_output_tokens: Some(64_000),
                },
            ],
        })
    }

    #[test]
    fn test_embedded_catalog_parses() {
        let index =
            CatalogIndex::from_json(MODEL_CATALOG_JSON).expect("embedded catalog should parse");
        assert!(!index.models.is_empty());
    }

    #[test]
    fn test_exact_id_match() {
        let index = test_index();
        let resolved = resolve_with_index(&index, "anthropic", "anthropic/claude-sonnet-4-5");
        assert_eq!(resolved.match_kind, MatchKind::Exact);
        assert_eq!(resolved.max_output_tokens, Some(64_000));
    }

    #[test]
    fn test_exact_canonical_slug_match() {
        let index = test_index();
        let resolved =
            resolve_with_index(&index, "anthropic", "anthropic/claude-sonnet-4-5-20250929");
        assert_eq!(resolved.match_kind, MatchKind::Exact);
        assert_eq!(
            resolved.matched_id.as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );
    }

    #[test]
    fn test_alias_match_with_provider_precedence() {
        let index = test_index();
        let resolved = resolve_with_index(&index, "anthropic", "claude_sonnet.4.5");
        assert_eq!(resolved.match_kind, MatchKind::Alias);
        assert_eq!(
            resolved.matched_id.as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );
    }

    #[test]
    fn test_fuzzy_alias_match_with_provider_precedence() {
        let index = test_index();
        let resolved = resolve_with_index(&index, "anthropic", "claude-sonnet-4-5-20250929");
        assert_eq!(resolved.match_kind, MatchKind::Alias);
        assert_eq!(
            resolved.matched_id.as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );
    }

    #[test]
    fn test_ambiguous_alias_returns_none() {
        let index = CatalogIndex::from_snapshot(Snapshot {
            models: vec![
                SnapshotModel {
                    id: "openai/gpt-4o".to_string(),
                    canonical_slug: None,
                    context_length: Some(128_000),
                    max_output_tokens: Some(16_384),
                },
                SnapshotModel {
                    id: "other/gpt-4o".to_string(),
                    canonical_slug: None,
                    context_length: Some(32_000),
                    max_output_tokens: Some(4_096),
                },
            ],
        });

        let resolved = resolve_with_index(&index, "unknown", "gpt_4o");
        assert_eq!(resolved.match_kind, MatchKind::None);
        assert!(resolved.matched_id.is_none());
    }
}
