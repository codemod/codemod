use rmcp::{handler::server::wrapper::Parameters, model::*, schemars, tool, ErrorData as McpError};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

const DEFAULT_KNOWLEDGE_LIMIT: usize = 5;
const MAX_KNOWLEDGE_LIMIT: usize = 20;
const JSSG_KNOWLEDGE_JSON: &str = include_str!("../data/knowledge/jssg-knowledge.json");
const AST_GREP_KNOWLEDGE_JSON: &str = include_str!("../data/knowledge/ast-grep-knowledge.json");

static JSSG_KNOWLEDGE_BASE: LazyLock<KnowledgeBase> =
    LazyLock::new(|| serde_json::from_str(JSSG_KNOWLEDGE_JSON).expect("valid JSSG knowledge JSON"));
static AST_GREP_KNOWLEDGE_BASE: LazyLock<KnowledgeBase> = LazyLock::new(|| {
    serde_json::from_str(AST_GREP_KNOWLEDGE_JSON).expect("valid ast-grep knowledge JSON")
});

#[derive(Debug, Deserialize)]
struct KnowledgeBase {
    version: String,
    #[serde(rename = "extractedAt")]
    extracted_at: String,
    sources: Vec<KnowledgeSource>,
    gotchas: Vec<KnowledgeEntry>,
    recipes: Vec<KnowledgeEntry>,
}

#[derive(Debug, Deserialize, Serialize, Clone, schemars::JsonSchema)]
pub struct KnowledgeSource {
    pub id: String,
    pub source_type: String,
    pub source_ref: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, schemars::JsonSchema)]
pub struct KnowledgeEntry {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub source_type: String,
    pub source_ref: String,
    pub verified: bool,
    pub tags: Vec<String>,
    pub content: String,
    #[serde(default)]
    pub bad_example: Option<String>,
    #[serde(default)]
    pub good_example: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct KnowledgeListRequest {
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchKnowledgeRequest {
    pub query: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct KnowledgeSearchResult {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub source_type: String,
    pub source_ref: String,
    pub verified: bool,
    pub tags: Vec<String>,
    pub content: String,
    pub bad_example: Option<String>,
    pub good_example: Option<String>,
    pub score: Option<i32>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct KnowledgeSearchResponse {
    pub knowledge_base: String,
    pub version: String,
    pub extracted_at: String,
    pub source_count: usize,
    pub total_matches: usize,
    pub entries: Vec<KnowledgeSearchResult>,
}

#[derive(Clone)]
pub struct KnowledgeHandler;

impl KnowledgeHandler {
    pub fn new() -> Self {
        Self
    }

    #[tool(
        description = "Get the highest-priority verified JSSG gotchas before implementing a codemod transform."
    )]
    pub async fn get_jssg_gotchas(
        &self,
        Parameters(request): Parameters<KnowledgeListRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.respond_with_list(
            "jssg",
            &JSSG_KNOWLEDGE_BASE,
            request.gotchas_request_limit(),
            &request.tags,
        )
    }

    #[tool(
        description = "Search the verified JSSG knowledge base for gotchas and recipes. Use this when implementing or repairing a codemod and you are unsure about a pattern or transform approach."
    )]
    pub async fn search_jssg_knowledge(
        &self,
        Parameters(request): Parameters<SearchKnowledgeRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.respond_with_search("jssg", &JSSG_KNOWLEDGE_BASE, request)
    }

    #[tool(
        description = "Get the highest-priority verified ast-grep gotchas before implementing a codemod transform."
    )]
    pub async fn get_ast_grep_gotchas(
        &self,
        Parameters(request): Parameters<KnowledgeListRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.respond_with_list(
            "ast-grep",
            &AST_GREP_KNOWLEDGE_BASE,
            request.gotchas_request_limit(),
            &request.tags,
        )
    }

    #[tool(
        description = "Search the verified ast-grep knowledge base for gotchas and recipes. Use this when a pattern is unclear or before considering regex or manual parsing fallbacks."
    )]
    pub async fn search_ast_grep_knowledge(
        &self,
        Parameters(request): Parameters<SearchKnowledgeRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.respond_with_search("ast-grep", &AST_GREP_KNOWLEDGE_BASE, request)
    }

    fn respond_with_list(
        &self,
        knowledge_base: &str,
        base: &KnowledgeBase,
        limit: usize,
        tags: &[String],
    ) -> Result<CallToolResult, McpError> {
        let entries = filter_by_tags(base.gotchas.iter().cloned().collect(), tags)
            .into_iter()
            .take(limit)
            .map(|entry| to_result(entry, None))
            .collect::<Vec<_>>();

        self.serialize_response(KnowledgeSearchResponse {
            knowledge_base: knowledge_base.to_string(),
            version: base.version.clone(),
            extracted_at: base.extracted_at.clone(),
            source_count: base.sources.len(),
            total_matches: entries.len(),
            entries,
        })
    }

    fn respond_with_search(
        &self,
        knowledge_base: &str,
        base: &KnowledgeBase,
        request: SearchKnowledgeRequest,
    ) -> Result<CallToolResult, McpError> {
        let limit = normalize_limit(request.limit);
        let query = request.query.trim().to_ascii_lowercase();
        let entries = filter_by_tags(
            base.gotchas
                .iter()
                .chain(base.recipes.iter())
                .cloned()
                .collect(),
            &request.tags,
        );

        let mut ranked = entries
            .into_iter()
            .filter_map(|entry| {
                let score = score_entry(&entry, &query);
                if score > 0 {
                    Some((score, entry))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        ranked.sort_by(|(left_score, left_entry), (right_score, right_entry)| {
            right_score
                .cmp(left_score)
                .then_with(|| left_entry.id.cmp(&right_entry.id))
        });

        let total_matches = ranked.len();
        let entries = ranked
            .into_iter()
            .take(limit)
            .map(|(score, entry)| to_result(entry, Some(score)))
            .collect::<Vec<_>>();

        self.serialize_response(KnowledgeSearchResponse {
            knowledge_base: knowledge_base.to_string(),
            version: base.version.clone(),
            extracted_at: base.extracted_at.clone(),
            source_count: base.sources.len(),
            total_matches,
            entries,
        })
    }

    fn serialize_response(
        &self,
        response: KnowledgeSearchResponse,
    ) -> Result<CallToolResult, McpError> {
        let content = serde_json::to_string_pretty(&response).map_err(|error| {
            McpError::internal_error(format!("Failed to serialize response: {error}"), None)
        })?;

        Ok(CallToolResult::success(vec![Content::text(content)]))
    }
}

fn filter_by_tags(entries: Vec<KnowledgeEntry>, tags: &[String]) -> Vec<KnowledgeEntry> {
    if tags.is_empty() {
        return entries;
    }

    let normalized = tags
        .iter()
        .map(|tag| tag.trim().to_ascii_lowercase())
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>();

    if normalized.is_empty() {
        return entries;
    }

    entries
        .into_iter()
        .filter(|entry| {
            entry.tags.iter().any(|tag| {
                let tag = tag.to_ascii_lowercase();
                normalized.iter().any(|wanted| wanted == &tag)
            })
        })
        .collect()
}

fn normalize_limit(limit: Option<usize>) -> usize {
    limit
        .unwrap_or(DEFAULT_KNOWLEDGE_LIMIT)
        .clamp(1, MAX_KNOWLEDGE_LIMIT)
}

fn score_entry(entry: &KnowledgeEntry, query: &str) -> i32 {
    let query_terms = query
        .split_whitespace()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();

    if query_terms.is_empty() {
        return 0;
    }

    let title = entry.title.to_ascii_lowercase();
    let summary = entry.summary.to_ascii_lowercase();
    let content = entry.content.to_ascii_lowercase();
    let tags = entry
        .tags
        .iter()
        .map(|tag| tag.to_ascii_lowercase())
        .collect::<Vec<_>>();

    let mut score = 0;
    let mut matched_terms = 0;

    for term in query_terms {
        let mut term_score = 0;
        if title.contains(term) {
            term_score += 5;
        }
        if summary.contains(term) {
            term_score += 4;
        }
        if tags.iter().any(|tag| tag.contains(term)) {
            term_score += 3;
        }
        if content.contains(term) {
            term_score += 2;
        }

        if term_score > 0 {
            matched_terms += 1;
            score += term_score;
        }
    }

    if matched_terms == 0 {
        return 0;
    }

    if matched_terms > 1 {
        score += matched_terms;
    }

    score
}

fn to_result(entry: KnowledgeEntry, score: Option<i32>) -> KnowledgeSearchResult {
    KnowledgeSearchResult {
        id: entry.id,
        title: entry.title,
        summary: entry.summary,
        source_type: entry.source_type,
        source_ref: entry.source_ref,
        verified: entry.verified,
        tags: entry.tags,
        content: entry.content,
        bad_example: entry.bad_example,
        good_example: entry.good_example,
        score,
    }
}

impl KnowledgeListRequest {
    fn gotchas_request_limit(&self) -> usize {
        normalize_limit(self.limit)
    }
}

impl Default for KnowledgeHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jssg_gotchas_are_verified() {
        assert!(JSSG_KNOWLEDGE_BASE.gotchas.iter().all(|entry| entry.verified));
        assert!(!JSSG_KNOWLEDGE_BASE.gotchas.is_empty());
    }

    #[test]
    fn ast_grep_search_finds_meta_variable_entry() {
        let entry = AST_GREP_KNOWLEDGE_BASE
            .gotchas
            .iter()
            .find(|entry| entry.id == "ast-grep-meta-variables-capture-whole-nodes")
            .expect("expected meta-variable gotcha");
        let score = score_entry(entry, "partial identifier meta variable");
        assert!(score > 0);
    }

    #[test]
    fn search_by_tags_filters_results() {
        let entries = filter_by_tags(
            JSSG_KNOWLEDGE_BASE.gotchas.clone(),
            &["metrics".to_string()],
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "jssg-keep-metrics-snapshots-in-sync");
    }

    #[test]
    fn normalize_limit_clamps_values() {
        assert_eq!(normalize_limit(None), DEFAULT_KNOWLEDGE_LIMIT);
        assert_eq!(normalize_limit(Some(0)), 1);
        assert_eq!(normalize_limit(Some(999)), MAX_KNOWLEDGE_LIMIT);
    }
}
