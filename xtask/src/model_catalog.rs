use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
struct OpenRouterModelsResponse {
    data: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ModelRecord {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    canonical_slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ModelCatalogSnapshot {
    schema_version: u32,
    generated_at: String,
    source_url: String,
    models: Vec<ModelRecord>,
}

fn parse_u64(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(raw) => raw.trim().parse::<u64>().ok(),
        _ => None,
    }
}

fn parse_model_record(value: &Value) -> Option<ModelRecord> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())?
        .to_string();

    let canonical_slug = value
        .get("canonical_slug")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|slug| !slug.is_empty())
        .map(ToOwned::to_owned);

    let context_length = parse_u64(value.get("context_length"));
    let max_output_tokens = parse_u64(
        value
            .get("top_provider")
            .and_then(Value::as_object)
            .and_then(|provider| provider.get("max_completion_tokens")),
    );

    Some(ModelRecord {
        id,
        canonical_slug,
        context_length,
        max_output_tokens,
    })
}

fn merge_records(existing: &mut ModelRecord, incoming: ModelRecord) {
    if existing.canonical_slug.is_none() {
        existing.canonical_slug = incoming.canonical_slug;
    }
    if existing.context_length.is_none() {
        existing.context_length = incoming.context_length;
    }
    if existing.max_output_tokens.is_none() {
        existing.max_output_tokens = incoming.max_output_tokens;
    }
}

fn build_snapshot(response: OpenRouterModelsResponse) -> Result<ModelCatalogSnapshot> {
    if response.data.is_empty() {
        bail!("OpenRouter models response is empty");
    }

    let mut by_id: BTreeMap<String, ModelRecord> = BTreeMap::new();
    for entry in &response.data {
        let Some(record) = parse_model_record(entry) else {
            continue;
        };

        by_id
            .entry(record.id.clone())
            .and_modify(|existing| merge_records(existing, record.clone()))
            .or_insert(record);
    }

    if by_id.is_empty() {
        bail!("No usable model records found in OpenRouter response");
    }

    Ok(ModelCatalogSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        source_url: OPENROUTER_MODELS_URL.to_string(),
        models: by_id.into_values().collect(),
    })
}

fn snapshot_output_path() -> Result<std::path::PathBuf> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .context("CARGO_MANIFEST_DIR is not set for xtask execution")?;
    let repo_root = Path::new(&manifest_dir)
        .parent()
        .ok_or_else(|| anyhow!("Failed to resolve repo root from xtask manifest dir"))?;

    Ok(repo_root.join("crates/ai/data/openrouter_models.json"))
}

fn load_existing_snapshot(path: &Path) -> Result<Option<ModelCatalogSnapshot>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("Failed to read existing snapshot from {}", path.display()))?;
    let snapshot = serde_json::from_str::<ModelCatalogSnapshot>(&raw).with_context(|| {
        format!(
            "Failed to parse existing model snapshot JSON at {}",
            path.display()
        )
    })?;
    Ok(Some(snapshot))
}

fn is_meaningful_change(existing: &ModelCatalogSnapshot, next: &ModelCatalogSnapshot) -> bool {
    existing.schema_version != next.schema_version
        || existing.source_url != next.source_url
        || existing.models != next.models
}

pub async fn update_model_catalog() -> Result<()> {
    let response = reqwest::Client::new()
        .get(OPENROUTER_MODELS_URL)
        .send()
        .await
        .context("Failed to fetch OpenRouter model catalog")?
        .error_for_status()
        .context("OpenRouter returned non-success status for model catalog fetch")?;

    let payload: OpenRouterModelsResponse = response
        .json()
        .await
        .context("Failed to parse OpenRouter model catalog response JSON")?;

    let snapshot = build_snapshot(payload)?;
    let output_path = snapshot_output_path()?;
    let parent = output_path
        .parent()
        .ok_or_else(|| anyhow!("Failed to resolve parent directory for snapshot output"))?;
    fs::create_dir_all(parent).context("Failed to create model snapshot output directory")?;

    if let Some(existing) = load_existing_snapshot(&output_path)? {
        if !is_meaningful_change(&existing, &snapshot) {
            println!(
                "OpenRouter model catalog unchanged ({} models). Skipping snapshot rewrite.",
                snapshot.models.len()
            );
            return Ok(());
        }
    }

    let mut serialized =
        serde_json::to_string_pretty(&snapshot).context("Failed to serialize model snapshot")?;
    serialized.push('\n');

    fs::write(&output_path, serialized).with_context(|| {
        format!(
            "Failed to write model snapshot to {}",
            output_path.display()
        )
    })?;

    println!(
        "Updated OpenRouter model catalog snapshot at {} ({} models)",
        output_path.display(),
        snapshot.models.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_snapshot_keeps_records_and_sorts_by_id() {
        let response = OpenRouterModelsResponse {
            data: vec![
                serde_json::json!({
                    "id": "z/model-b",
                    "context_length": 1000,
                    "top_provider": {"max_completion_tokens": 400}
                }),
                serde_json::json!({
                    "id": "a/model-a",
                    "canonical_slug": "a/model-a-20250301",
                    "context_length": 2000
                }),
            ],
        };

        let snapshot = build_snapshot(response).expect("snapshot should build");
        assert_eq!(snapshot.models.len(), 2);
        assert_eq!(snapshot.models[0].id, "a/model-a");
        assert_eq!(snapshot.models[1].id, "z/model-b");
        assert_eq!(snapshot.models[1].max_output_tokens, Some(400));
    }

    #[test]
    fn test_parse_u64_supports_number_and_string() {
        assert_eq!(parse_u64(Some(&serde_json::json!(123))), Some(123));
        assert_eq!(parse_u64(Some(&serde_json::json!("456"))), Some(456));
        assert_eq!(parse_u64(Some(&serde_json::json!("not-a-number"))), None);
    }
}
