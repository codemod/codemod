use butterflow_models::step::{BuiltinShardMethod, BuiltinShardType, ShardMethod, UseShard};
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Result of shard evaluation — one entry per shard, written to workflow state.
///
/// Fields prefixed with `_meta_` are excluded from matrix hashing in the scheduler,
/// so changing `_meta_files` or `_meta_shard` won't cause the scheduler to treat
/// an existing shard as a new one.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ShardResult {
    /// Human-readable shard name (used in PR titles, branch names).
    /// This is part of the shard's identity hash.
    pub name: String,
    /// Numeric shard index (0-based). Excluded from hashing via `_meta_` prefix.
    pub _meta_shard: usize,
    /// File paths belonging to this shard (consumed by matrix `_meta_files` filtering).
    /// Excluded from hashing via `_meta_` prefix.
    pub _meta_files: Vec<String>,
    /// Subdirectory path (set by `directory` method). Part of identity hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    /// CODEOWNERS team (set by `codeowner` method). Part of identity hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
}

/// Evaluate shards using the built-in method specified in the step config.
///
/// - `eligible_files`: When provided (from a codemod pre-scan), only those files
///   are considered. When `None`, all files matching `file_pattern` under `target` are used.
/// - `previous_shards`: When provided (from current state), evaluation is incremental:
///   existing file→shard assignments are preserved, and only new files get new shards.
///   This guarantees stability across re-evaluations.
pub fn evaluate_builtin_shards(
    shard_config: &UseShard,
    target_path: &Path,
    eligible_files: Option<&[String]>,
    previous_shards: Option<&Vec<serde_json::Value>>,
) -> Result<Vec<ShardResult>, String> {
    let method = match &shard_config.method {
        ShardMethod::Builtin(builtin) => builtin,
        ShardMethod::Function(_) => {
            return Err("evaluate_builtin_shards called with custom function method".to_string());
        }
    };

    let target = shard_config
        .target
        .as_deref()
        .ok_or("target is required for built-in shard methods")?;

    // Resolve target relative to the working directory
    let search_base = if Path::new(target).is_absolute() {
        PathBuf::from(target)
    } else {
        target_path.join(target)
    };

    if !search_base.exists() {
        return Err(format!(
            "Shard target directory does not exist: {}",
            search_base.display()
        ));
    }

    // Collect current files — either pre-filtered or from glob
    let relative_files: Vec<String> = if let Some(eligible) = eligible_files {
        eligible.to_vec()
    } else {
        let file_pattern = shard_config
            .file_pattern
            .as_deref()
            .ok_or("file_pattern is required when js-ast-grep is not set")?;
        let files = collect_files_with_pattern(&search_base, file_pattern)?;
        files
            .iter()
            .filter_map(|f| {
                f.strip_prefix(target_path)
                    .ok()
                    .map(|p| p.to_string_lossy().to_string())
            })
            .collect()
    };

    if relative_files.is_empty() && previous_shards.is_none() {
        return Ok(Vec::new());
    }

    let current_file_set: HashSet<&str> = relative_files.iter().map(|s| s.as_str()).collect();

    // If we have previous shards, do incremental evaluation
    if let Some(prev) = previous_shards {
        if !prev.is_empty() {
            return evaluate_incremental(
                prev,
                &current_file_set,
                method,
                &search_base,
                target_path,
            );
        }
    }

    // Fresh evaluation — no previous state
    if relative_files.is_empty() {
        return Ok(Vec::new());
    }

    match method.r#type {
        BuiltinShardType::Directory => evaluate_directory_shards(
            &relative_files,
            &search_base,
            target_path,
            method.max_files_per_shard,
            method.min_shard_size,
        ),
        BuiltinShardType::Codeowner => evaluate_codeowner_shards(
            &relative_files,
            target_path,
            method.max_files_per_shard,
            method.min_shard_size,
        ),
    }
}

// ── Incremental evaluation ───────────────────────────────────────────────

/// Incremental shard evaluation: preserves existing file→shard assignments and
/// only creates new shards for files that weren't in any previous shard.
///
/// Guarantees:
/// - Files already assigned to a shard stay in that shard (stable identity)
/// - New files are grouped and bin-packed into new shards
/// - Previous shards with all files deleted are dropped (scheduler marks them WontDo)
fn evaluate_incremental(
    previous: &[serde_json::Value],
    current_files: &HashSet<&str>,
    method: &BuiltinShardMethod,
    search_base: &Path,
    target_path: &Path,
) -> Result<Vec<ShardResult>, String> {
    // 1. Parse previous shards and track claimed files
    let mut kept_shards: Vec<ShardResult> = Vec::new();
    let mut claimed_files: HashSet<String> = HashSet::new();
    // Track highest chunk index per group for generating new shard names
    let mut max_chunk_index: HashMap<String, usize> = HashMap::new();

    for prev_value in previous {
        let prev: ShardResult = serde_json::from_value(prev_value.clone())
            .map_err(|e| format!("Failed to parse previous shard: {e}"))?;

        // Determine the group key for tracking chunk indices
        let group_key = prev
            .directory
            .clone()
            .or_else(|| prev.team.clone())
            .unwrap_or_else(|| ".".to_string());

        // Parse the chunk index from the name (e.g., "components-2" → 2)
        if let Some(idx) = parse_chunk_index(&prev.name) {
            let entry = max_chunk_index.entry(group_key.clone()).or_insert(0);
            *entry = (*entry).max(idx);
        }

        // Filter to files that still exist
        let surviving_files: Vec<String> = prev
            ._meta_files
            .iter()
            .filter(|f| current_files.contains(f.as_str()))
            .cloned()
            .collect();

        if surviving_files.is_empty() {
            // All files deleted → drop the shard (scheduler will WontDo the task)
            continue;
        }

        // Claim surviving files
        for f in &surviving_files {
            claimed_files.insert(f.clone());
        }

        kept_shards.push(ShardResult {
            name: prev.name,
            _meta_shard: 0, // re-indexed later
            _meta_files: surviving_files,
            directory: prev.directory,
            team: prev.team,
        });
    }

    // 2. Find new files (in current set but not claimed by any previous shard)
    let mut new_files: Vec<String> = current_files
        .iter()
        .filter(|f| !claimed_files.contains(**f))
        .map(|f| f.to_string())
        .collect();
    new_files.sort();

    // 3. Create new shards for new files
    if !new_files.is_empty() {
        let new_shards = match method.r#type {
            BuiltinShardType::Directory => create_new_directory_shards(
                &new_files,
                search_base,
                target_path,
                method.max_files_per_shard,
                method.min_shard_size,
                &max_chunk_index,
            ),
            BuiltinShardType::Codeowner => create_new_codeowner_shards(
                &new_files,
                target_path,
                method.max_files_per_shard,
                method.min_shard_size,
                &max_chunk_index,
            ),
        }?;
        kept_shards.extend(new_shards);
    }

    // 4. Re-index _meta_shard
    for (i, shard) in kept_shards.iter_mut().enumerate() {
        shard._meta_shard = i;
    }

    Ok(kept_shards)
}

/// Parse the trailing chunk index from a shard name like "components-2" → Some(2).
fn parse_chunk_index(name: &str) -> Option<usize> {
    name.rsplit('-').next()?.parse().ok()
}

/// Create new shards for directory-grouped files, starting chunk indices after
/// the highest existing index per directory.
fn create_new_directory_shards(
    new_files: &[String],
    search_base: &Path,
    target_path: &Path,
    max_files_per_shard: usize,
    min_shard_size: Option<usize>,
    max_chunk_index: &HashMap<String, usize>,
) -> Result<Vec<ShardResult>, String> {
    let search_base_rel = search_base.strip_prefix(target_path).unwrap_or(search_base);

    // Group new files by directory
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    for file in new_files {
        let group_key = extract_directory_group(file, search_base_rel);
        groups.entry(group_key).or_default().push(file.clone());
    }

    let mut group_names: Vec<String> = groups.keys().cloned().collect();
    group_names.sort();

    let mut shards = Vec::new();
    for group_name in &group_names {
        let mut files = groups.get(group_name).cloned().unwrap_or_default();
        files.sort();

        let chunks = bin_pack_files(&files, max_files_per_shard, min_shard_size);
        let start_index = max_chunk_index.get(group_name).map(|i| i + 1).unwrap_or(0);

        for (i, shard_files) in chunks.into_iter().enumerate() {
            let chunk_idx = start_index + i;
            let name = if group_names.len() == 1 && group_name == "." {
                format!("shard-{chunk_idx}")
            } else {
                format!("{group_name}-{chunk_idx}")
            };

            shards.push(ShardResult {
                name,
                _meta_shard: 0, // re-indexed by caller
                _meta_files: shard_files,
                directory: Some(group_name.clone()),
                team: None,
            });
        }
    }

    Ok(shards)
}

/// Create new shards for codeowner-grouped files, starting chunk indices after
/// the highest existing index per team.
/// If no CODEOWNERS file is found, all files are assigned to the "unowned" team.
fn create_new_codeowner_shards(
    new_files: &[String],
    target_path: &Path,
    max_files_per_shard: usize,
    min_shard_size: Option<usize>,
    max_chunk_index: &HashMap<String, usize>,
) -> Result<Vec<ShardResult>, String> {
    let rules = match find_codeowners_file(target_path) {
        Ok(content) => parse_codeowners(&content)?,
        Err(_) => Vec::new(),
    };

    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    for file in new_files {
        let team = match_codeowner(file, &rules).unwrap_or_else(|| "unowned".to_string());
        groups.entry(team).or_default().push(file.clone());
    }

    let mut team_names: Vec<String> = groups.keys().cloned().collect();
    team_names.sort();

    let mut shards = Vec::new();
    for team_name in &team_names {
        let mut files = groups.get(team_name).cloned().unwrap_or_default();
        files.sort();

        let chunks = bin_pack_files(&files, max_files_per_shard, min_shard_size);
        let sanitized = sanitize_team_name(team_name);
        let start_index = max_chunk_index.get(team_name).map(|i| i + 1).unwrap_or(0);

        for (i, shard_files) in chunks.into_iter().enumerate() {
            let chunk_idx = start_index + i;
            let name = format!("{sanitized}-{chunk_idx}");

            shards.push(ShardResult {
                name,
                _meta_shard: 0,
                _meta_files: shard_files,
                directory: None,
                team: Some(team_name.clone()),
            });
        }
    }

    Ok(shards)
}

// ── Fresh evaluation (first run, no previous state) ─────────────────────

/// Collect files matching a glob pattern under a base path, respecting .gitignore.
pub fn collect_files_with_pattern(
    base_path: &Path,
    file_pattern: &str,
) -> Result<Vec<PathBuf>, String> {
    let mut override_builder = OverrideBuilder::new(base_path);
    override_builder
        .add(file_pattern)
        .map_err(|e| format!("Invalid file_pattern '{}': {}", file_pattern, e))?;
    let overrides = override_builder
        .build()
        .map_err(|e| format!("Failed to build glob overrides: {}", e))?;

    let mut builder = WalkBuilder::new(base_path);
    builder
        .overrides(overrides)
        .follow_links(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .parents(true)
        .ignore(true)
        .hidden(false);

    let walker = builder.threads(1).build();
    let mut files = Vec::new();

    for entry in walker {
        match entry {
            Ok(dir_entry) => {
                if dir_entry.file_type().is_some_and(|ft| ft.is_file()) {
                    files.push(dir_entry.into_path());
                }
            }
            Err(err) => {
                eprintln!("Walk error during shard evaluation: {}", err);
            }
        }
    }

    // Sort for deterministic output
    files.sort();
    Ok(files)
}

/// Extract directory group key for a file path relative to the search base.
fn extract_directory_group(file: &str, search_base_rel: &Path) -> String {
    let file_path = Path::new(file);
    let relative_to_search = if let Ok(rel) = file_path.strip_prefix(search_base_rel) {
        rel
    } else {
        file_path
    };

    // If the first component is the file itself (file at root of target), group as "."
    if relative_to_search.components().count() <= 1 {
        ".".to_string()
    } else {
        relative_to_search
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string())
    }
}

/// Directory sharding: group files by immediate subdirectory under target, then bin-pack.
fn evaluate_directory_shards(
    relative_files: &[String],
    search_base: &Path,
    target_path: &Path,
    max_files_per_shard: usize,
    min_shard_size: Option<usize>,
) -> Result<Vec<ShardResult>, String> {
    let search_base_rel = search_base.strip_prefix(target_path).unwrap_or(search_base);

    // Group files by their immediate subdirectory under target
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();

    for file in relative_files {
        let group_key = extract_directory_group(file, search_base_rel);
        groups.entry(group_key).or_default().push(file.clone());
    }

    // Sort groups by name for deterministic output
    let mut group_names: Vec<String> = groups.keys().cloned().collect();
    group_names.sort();

    let mut shards = Vec::new();

    for group_name in &group_names {
        let mut files = groups.get(group_name).cloned().unwrap_or_default();
        files.sort();

        let group_shards = bin_pack_files(&files, max_files_per_shard, min_shard_size);

        for (i, shard_files) in group_shards.into_iter().enumerate() {
            let name = if group_names.len() == 1 && group_name == "." {
                format!("shard-{}", i)
            } else {
                format!("{}-{}", group_name, i)
            };

            shards.push(ShardResult {
                name,
                _meta_shard: shards.len(),
                _meta_files: shard_files,
                directory: Some(group_name.clone()),
                team: None,
            });
        }
    }

    Ok(shards)
}

/// CODEOWNERS sharding: group files by owning team, then bin-pack.
/// If no CODEOWNERS file is found, all files are assigned to the "unowned" team.
fn evaluate_codeowner_shards(
    relative_files: &[String],
    target_path: &Path,
    max_files_per_shard: usize,
    min_shard_size: Option<usize>,
) -> Result<Vec<ShardResult>, String> {
    let rules = match find_codeowners_file(target_path) {
        Ok(content) => parse_codeowners(&content)?,
        Err(_) => Vec::new(),
    };

    // Group files by owning team
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();

    for file in relative_files {
        let team = match_codeowner(file, &rules).unwrap_or_else(|| "unowned".to_string());
        groups.entry(team).or_default().push(file.clone());
    }

    // Sort groups by team name for deterministic output
    let mut team_names: Vec<String> = groups.keys().cloned().collect();
    team_names.sort();

    let mut shards = Vec::new();

    for team_name in &team_names {
        let mut files = groups.get(team_name).cloned().unwrap_or_default();
        files.sort();

        let group_shards = bin_pack_files(&files, max_files_per_shard, min_shard_size);

        for (i, shard_files) in group_shards.into_iter().enumerate() {
            let name = format!("{}-{}", sanitize_team_name(team_name), i);

            shards.push(ShardResult {
                name,
                _meta_shard: shards.len(),
                _meta_files: shard_files,
                directory: None,
                team: Some(team_name.clone()),
            });
        }
    }

    Ok(shards)
}

/// Bin-pack files into chunks of at most `max_size`, merging trailing runts
/// smaller than `min_size` into the previous chunk.
fn bin_pack_files(files: &[String], max_size: usize, min_size: Option<usize>) -> Vec<Vec<String>> {
    if files.is_empty() || max_size == 0 {
        return Vec::new();
    }

    let mut chunks: Vec<Vec<String>> = files.chunks(max_size).map(|c| c.to_vec()).collect();

    // Merge trailing runt into previous chunk
    if let Some(min) = min_size {
        if chunks.len() > 1 {
            let last_len = chunks.last().map(|c| c.len()).unwrap_or(0);
            if last_len < min {
                let last = chunks.pop().unwrap();
                if let Some(prev) = chunks.last_mut() {
                    prev.extend(last);
                }
            }
        }
    }

    chunks
}

// ── CODEOWNERS parsing ──────────────────────────────────────────────────

/// A single CODEOWNERS rule: a glob pattern and list of owners.
#[derive(Debug, Clone)]
struct CodeownersRule {
    pattern: String,
    owners: Vec<String>,
}

/// Find the CODEOWNERS file in standard locations.
fn find_codeowners_file(repo_root: &Path) -> Result<String, String> {
    let candidates = [
        repo_root.join(".github/CODEOWNERS"),
        repo_root.join("CODEOWNERS"),
        repo_root.join("docs/CODEOWNERS"),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return std::fs::read_to_string(candidate)
                .map_err(|e| format!("Failed to read {}: {}", candidate.display(), e));
        }
    }

    Err(format!(
        "No CODEOWNERS file found in {}. Checked .github/CODEOWNERS, CODEOWNERS, docs/CODEOWNERS",
        repo_root.display()
    ))
}

/// Parse a CODEOWNERS file into a list of rules.
/// Rules are returned in file order — later rules take precedence (last match wins).
fn parse_codeowners(content: &str) -> Result<Vec<CodeownersRule>, String> {
    let mut rules = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Skip comments and empty lines
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 2 {
            // A pattern with no owners — skip
            continue;
        }

        let pattern = parts[0].to_string();
        let owners: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();

        rules.push(CodeownersRule { pattern, owners });
    }

    Ok(rules)
}

/// Match a file path against CODEOWNERS rules (last match wins, per GitHub spec).
/// Returns the first owner (team) of the matching rule.
fn match_codeowner(file_path: &str, rules: &[CodeownersRule]) -> Option<String> {
    let mut matched_owner: Option<String> = None;

    for rule in rules {
        if codeowner_pattern_matches(&rule.pattern, file_path) {
            // Use the first owner listed in the rule
            matched_owner = rule.owners.first().cloned();
        }
    }

    matched_owner
}

/// Check if a CODEOWNERS pattern matches a file path.
/// CODEOWNERS patterns follow gitignore-like semantics.
fn codeowner_pattern_matches(pattern: &str, file_path: &str) -> bool {
    let pattern = pattern.trim_start_matches('/');
    let file_path = file_path.trim_start_matches('/');

    // Convert CODEOWNERS pattern to a simple glob match
    // Handle common cases: exact path, directory prefix, wildcard patterns
    if pattern == "*" {
        return true;
    }

    // If pattern ends with /, it matches all files under that directory
    if pattern.ends_with('/') {
        let dir_pattern = pattern.trim_end_matches('/');
        return file_path.starts_with(dir_pattern)
            && file_path
                .get(dir_pattern.len()..)
                .is_some_and(|rest| rest.starts_with('/'));
    }

    // If pattern contains no slash (other than leading), it matches anywhere in the tree
    if !pattern.contains('/') {
        if pattern.contains('*') {
            // Match against each path segment (filename or directory name)
            for segment in file_path.split('/') {
                if segment_matches(pattern, segment) {
                    return true;
                }
            }
            return false;
        }
        // Match as a directory prefix or exact filename
        return file_path == pattern
            || file_path.starts_with(&format!("{}/", pattern))
            || file_path.ends_with(&format!("/{}", pattern));
    }

    // Pattern contains a slash — match from root
    if pattern.contains('*') {
        return glob_simple_match(pattern, file_path);
    }

    // Exact prefix or exact match
    file_path == pattern || file_path.starts_with(&format!("{}/", pattern))
}

/// Simple glob matching supporting `*` (one segment) and `**` (multiple segments).
fn glob_simple_match(pattern: &str, path: &str) -> bool {
    // Split into segments
    let pattern_parts: Vec<&str> = pattern.split('/').collect();
    let path_parts: Vec<&str> = path.split('/').collect();

    glob_match_segments(&pattern_parts, &path_parts)
}

fn glob_match_segments(pattern: &[&str], path: &[&str]) -> bool {
    if pattern.is_empty() {
        // Pattern exhausted — match if path is also exhausted or we match a subtree
        return true;
    }

    if path.is_empty() {
        // Path exhausted but pattern remains — only match if rest is all **
        return pattern.iter().all(|p| *p == "**");
    }

    let p = pattern[0];

    if p == "**" {
        // ** matches zero or more path segments
        // Try matching zero segments, then one, then two, etc.
        for i in 0..=path.len() {
            if glob_match_segments(&pattern[1..], &path[i..]) {
                return true;
            }
        }
        return false;
    }

    // Match current segment
    if segment_matches(p, path[0]) {
        return glob_match_segments(&pattern[1..], &path[1..]);
    }

    false
}

/// Match a single path segment against a pattern segment with `*` wildcards.
fn segment_matches(pattern: &str, segment: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') {
        return pattern == segment;
    }

    // Simple wildcard matching within a segment
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 2 {
        let (prefix, suffix) = (parts[0], parts[1]);
        return segment.starts_with(prefix)
            && segment.ends_with(suffix)
            && segment.len() >= prefix.len() + suffix.len();
    }

    // Multiple wildcards — use recursive matching
    wildcard_match(pattern, segment)
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let mut dp = vec![vec![false; t.len() + 1]; p.len() + 1];
    dp[0][0] = true;

    for i in 1..=p.len() {
        if p[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }

    for i in 1..=p.len() {
        for j in 1..=t.len() {
            if p[i - 1] == '*' {
                dp[i][j] = dp[i - 1][j] || dp[i][j - 1];
            } else if p[i - 1] == t[j - 1] || p[i - 1] == '?' {
                dp[i][j] = dp[i - 1][j - 1];
            }
        }
    }

    dp[p.len()][t.len()]
}

/// Sanitize a team name for use in branch/shard names.
fn sanitize_team_name(team: &str) -> String {
    team.trim_start_matches('@')
        .replace(['/', ' '], "-")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── bin_pack_files ──────────────────────────────────────────────

    #[test]
    fn test_bin_pack_exact_fit() {
        let files: Vec<String> = (0..10).map(|i| format!("file-{}.tsx", i)).collect();
        let result = bin_pack_files(&files, 5, None);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].len(), 5);
        assert_eq!(result[1].len(), 5);
    }

    #[test]
    fn test_bin_pack_with_remainder() {
        let files: Vec<String> = (0..7).map(|i| format!("file-{}.tsx", i)).collect();
        let result = bin_pack_files(&files, 5, None);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].len(), 5);
        assert_eq!(result[1].len(), 2);
    }

    #[test]
    fn test_bin_pack_merge_runt() {
        let files: Vec<String> = (0..12).map(|i| format!("file-{}.tsx", i)).collect();
        // 12 files, max 5 → chunks [5, 5, 2]. min_shard_size=3 → merge last into previous: [5, 7]
        let result = bin_pack_files(&files, 5, Some(3));
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].len(), 5);
        assert_eq!(result[1].len(), 7);
    }

    #[test]
    fn test_bin_pack_runt_above_min() {
        let files: Vec<String> = (0..13).map(|i| format!("file-{}.tsx", i)).collect();
        // 13 files, max 5 → chunks [5, 5, 3]. min_shard_size=3 → no merge since 3 >= 3
        let result = bin_pack_files(&files, 5, Some(3));
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_bin_pack_empty() {
        let result = bin_pack_files(&[], 5, None);
        assert!(result.is_empty());
    }

    #[test]
    fn test_bin_pack_single_file() {
        let files = vec!["a.tsx".to_string()];
        let result = bin_pack_files(&files, 5, Some(3));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].len(), 1);
    }

    // ── parse_chunk_index ─────────────────────────────────────────

    #[test]
    fn test_parse_chunk_index() {
        assert_eq!(parse_chunk_index("components-0"), Some(0));
        assert_eq!(parse_chunk_index("components-2"), Some(2));
        assert_eq!(parse_chunk_index("shard-5"), Some(5));
        assert_eq!(parse_chunk_index("org-frontend-team-1"), Some(1));
        assert_eq!(parse_chunk_index("no-number-here"), None);
    }

    // ── incremental evaluation ────────────────────────────────────

    #[test]
    fn test_incremental_preserves_existing_assignments() {
        // Previous shards: components-0 has [a.tsx, b.tsx], components-1 has [c.tsx]
        let previous = vec![
            serde_json::json!({
                "name": "components-0",
                "_meta_shard": 0,
                "_meta_files": ["src/components/a.tsx", "src/components/b.tsx"],
                "directory": "components"
            }),
            serde_json::json!({
                "name": "components-1",
                "_meta_shard": 1,
                "_meta_files": ["src/components/c.tsx"],
                "directory": "components"
            }),
        ];

        // Current files: same as before + one new file
        let current: HashSet<&str> = [
            "src/components/a.tsx",
            "src/components/b.tsx",
            "src/components/c.tsx",
            "src/components/d.tsx", // new
        ]
        .into_iter()
        .collect();

        let method = BuiltinShardMethod {
            r#type: BuiltinShardType::Directory,
            max_files_per_shard: 3,
            min_shard_size: None,
        };

        let result = evaluate_incremental(
            &previous,
            &current,
            &method,
            Path::new("/repo/src"),
            Path::new("/repo"),
        )
        .unwrap();

        // Should have 3 shards: 2 original + 1 new for d.tsx
        assert_eq!(result.len(), 3);

        // Original shards preserved exactly
        assert_eq!(result[0].name, "components-0");
        assert_eq!(
            result[0]._meta_files,
            vec!["src/components/a.tsx", "src/components/b.tsx"]
        );

        assert_eq!(result[1].name, "components-1");
        assert_eq!(result[1]._meta_files, vec!["src/components/c.tsx"]);

        // New shard gets next index
        assert_eq!(result[2].name, "components-2");
        assert_eq!(result[2]._meta_files, vec!["src/components/d.tsx"]);
    }

    #[test]
    fn test_incremental_drops_empty_shards() {
        let previous = vec![
            serde_json::json!({
                "name": "api-0",
                "_meta_shard": 0,
                "_meta_files": ["src/api/handler.ts"],
                "directory": "api"
            }),
            serde_json::json!({
                "name": "utils-0",
                "_meta_shard": 1,
                "_meta_files": ["src/utils/deleted.ts"],
                "directory": "utils"
            }),
        ];

        // Only the api file still exists — utils file was deleted
        let current: HashSet<&str> = ["src/api/handler.ts"].into_iter().collect();

        let method = BuiltinShardMethod {
            r#type: BuiltinShardType::Directory,
            max_files_per_shard: 10,
            min_shard_size: None,
        };

        let result = evaluate_incremental(
            &previous,
            &current,
            &method,
            Path::new("/repo/src"),
            Path::new("/repo"),
        )
        .unwrap();

        // Only 1 shard (utils-0 dropped because its file was deleted)
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "api-0");
    }

    #[test]
    fn test_incremental_new_directory_gets_index_zero() {
        let previous = vec![serde_json::json!({
            "name": "api-0",
            "_meta_shard": 0,
            "_meta_files": ["src/api/handler.ts"],
            "directory": "api"
        })];

        // Existing file + new file in a new directory
        let current: HashSet<&str> = ["src/api/handler.ts", "src/hooks/useAuth.ts"]
            .into_iter()
            .collect();

        let method = BuiltinShardMethod {
            r#type: BuiltinShardType::Directory,
            max_files_per_shard: 10,
            min_shard_size: None,
        };

        let result = evaluate_incremental(
            &previous,
            &current,
            &method,
            Path::new("/repo/src"),
            Path::new("/repo"),
        )
        .unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "api-0");
        assert_eq!(result[1].name, "hooks-0"); // new directory starts at 0
    }

    #[test]
    fn test_incremental_deleted_file_removed_from_shard() {
        let previous = vec![serde_json::json!({
            "name": "api-0",
            "_meta_shard": 0,
            "_meta_files": ["src/api/a.ts", "src/api/b.ts", "src/api/c.ts"],
            "directory": "api"
        })];

        // b.ts was deleted
        let current: HashSet<&str> = ["src/api/a.ts", "src/api/c.ts"].into_iter().collect();

        let method = BuiltinShardMethod {
            r#type: BuiltinShardType::Directory,
            max_files_per_shard: 10,
            min_shard_size: None,
        };

        let result = evaluate_incremental(
            &previous,
            &current,
            &method,
            Path::new("/repo/src"),
            Path::new("/repo"),
        )
        .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "api-0");
        // Deleted file removed from _meta_files
        assert_eq!(result[0]._meta_files, vec!["src/api/a.ts", "src/api/c.ts"]);
    }

    #[test]
    fn test_incremental_meta_shard_reindexed() {
        let previous = vec![
            serde_json::json!({
                "name": "api-0",
                "_meta_shard": 0,
                "_meta_files": ["src/api/a.ts"],
                "directory": "api"
            }),
            serde_json::json!({
                "name": "utils-0",
                "_meta_shard": 1,
                "_meta_files": ["src/utils/deleted.ts"],
                "directory": "utils"
            }),
            serde_json::json!({
                "name": "hooks-0",
                "_meta_shard": 2,
                "_meta_files": ["src/hooks/b.ts"],
                "directory": "hooks"
            }),
        ];

        // utils file deleted, so utils-0 is dropped, hooks-0 gets re-indexed to 1
        let current: HashSet<&str> = ["src/api/a.ts", "src/hooks/b.ts"].into_iter().collect();

        let method = BuiltinShardMethod {
            r#type: BuiltinShardType::Directory,
            max_files_per_shard: 10,
            min_shard_size: None,
        };

        let result = evaluate_incremental(
            &previous,
            &current,
            &method,
            Path::new("/repo/src"),
            Path::new("/repo"),
        )
        .unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0]._meta_shard, 0);
        assert_eq!(result[0].name, "api-0");
        assert_eq!(result[1]._meta_shard, 1);
        assert_eq!(result[1].name, "hooks-0");
    }

    // ── CODEOWNERS parsing ──────────────────────────────────────────

    #[test]
    fn test_parse_codeowners() {
        let content = r#"
# This is a comment
*.js @frontend-team
/docs/ @docs-team
src/api/ @backend-team @api-team
"#;
        let rules = parse_codeowners(content).unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].pattern, "*.js");
        assert_eq!(rules[0].owners, vec!["@frontend-team"]);
        assert_eq!(rules[2].owners, vec!["@backend-team", "@api-team"]);
    }

    #[test]
    fn test_codeowner_pattern_matches_wildcard() {
        assert!(codeowner_pattern_matches("*", "anything/at/all.tsx"));
    }

    #[test]
    fn test_codeowner_pattern_matches_directory() {
        assert!(codeowner_pattern_matches("src/api/", "src/api/handler.ts"));
        assert!(!codeowner_pattern_matches(
            "src/api/",
            "src/apiary/handler.ts"
        ));
    }

    #[test]
    fn test_codeowner_pattern_matches_glob() {
        assert!(codeowner_pattern_matches(
            "*.tsx",
            "src/components/Button.tsx"
        ));
        assert!(!codeowner_pattern_matches("*.tsx", "src/utils/helper.ts"));
    }

    #[test]
    fn test_codeowner_pattern_matches_exact() {
        assert!(codeowner_pattern_matches("src/config.ts", "src/config.ts"));
        assert!(!codeowner_pattern_matches(
            "src/config.ts",
            "src/config.tsx"
        ));
    }

    #[test]
    fn test_codeowner_last_match_wins() {
        let rules =
            parse_codeowners("* @default-team\nsrc/ @src-team\nsrc/api/ @api-team").unwrap();

        assert_eq!(
            match_codeowner("src/api/handler.ts", &rules),
            Some("@api-team".to_string())
        );
        assert_eq!(
            match_codeowner("src/utils/helper.ts", &rules),
            Some("@src-team".to_string())
        );
        assert_eq!(
            match_codeowner("README.md", &rules),
            Some("@default-team".to_string())
        );
    }

    #[test]
    fn test_codeowner_double_star() {
        assert!(codeowner_pattern_matches(
            "src/**/test.ts",
            "src/a/b/test.ts"
        ));
        assert!(codeowner_pattern_matches("src/**/test.ts", "src/test.ts"));
    }

    // ── sanitize_team_name ──────────────────────────────────────────

    #[test]
    fn test_sanitize_team_name() {
        assert_eq!(
            sanitize_team_name("@org/frontend-team"),
            "org-frontend-team"
        );
        assert_eq!(sanitize_team_name("@backend"), "backend");
        assert_eq!(sanitize_team_name("unowned"), "unowned");
    }
}
