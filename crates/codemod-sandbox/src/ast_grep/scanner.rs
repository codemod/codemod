use std::{borrow::Cow, panic};

use ast_grep_config::CombinedScan;
use ast_grep_core::{replacer::Content, AstGrep, Doc, Language};

use crate::ast_grep::types::{AstGrepError, AstGrepMatch};

pub(crate) struct ScanResult<'a> {
    pub matches: Vec<AstGrepMatch>,
    pub file_modified: bool,
    pub new_content: Cow<'a, str>,
}

pub(crate) fn scan_content<'a, D: Doc<Lang = L>, L: Language>(
    root: &AstGrep<D>,
    content: &'a str,
    file_path: String,
    combined_scan: &CombinedScan<L>,
    apply_fixes: bool,
) -> Result<ScanResult<'a>, AstGrepError>
where
    <D as Doc>::Source: Content<Underlying = u8>,
{
    let scan_result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        combined_scan.scan(root, apply_fixes)
    }));

    let scan_result = scan_result.map_err(|_| {
        AstGrepError::Config(format!(
            "AST-grep rule configuration error for file: {file_path}. This usually indicates improper ellipsis patterns or rule syntax."
        ))
    })?;
    let mut matches = Vec::new();
    let file_modified = !scan_result.diffs.is_empty();
    let mut new_content = Cow::Borrowed(content);

    // Handle diffs (rules with fixers) when applying fixes
    if apply_fixes && !scan_result.diffs.is_empty() {
        // Collect all edits and sort them in reverse order (from end to start)
        // to avoid offset issues when applying multiple fixes
        let mut edit_infos: Vec<(usize, usize, Vec<u8>)> = Vec::new(); // (position, deleted_length, inserted_text)

        for (rule, node_match) in &scan_result.diffs {
            if let Ok(fixers) = rule.get_fixer() {
                if let Some(fixer) = fixers.first() {
                    let edit = node_match.make_edit(&rule.matcher, fixer);
                    edit_infos.push((edit.position, edit.deleted_length, edit.inserted_text));
                }
            }

            // Also record this as a match for reporting
            let node = node_match.get_node();
            let start_pos = node.start_pos();
            let end_pos = node.end_pos();

            matches.push(AstGrepMatch {
                file_path: file_path.clone(),
                start_byte: node.range().start,
                end_byte: node.range().end,
                start_line: start_pos.line(),
                start_column: start_pos.column(node),
                end_line: end_pos.line(),
                end_column: end_pos.column(node),
                match_text: node.text().to_string(),
                rule_id: rule.id.clone(),
            });
        }

        // Sort edits by position in reverse order (end to start)
        edit_infos.sort_by(|a, b| b.0.cmp(&a.0));

        // Apply edits to content using a proper offset-tracking approach
        if !edit_infos.is_empty() {
            // Build the new content by applying edits in reverse order (end to start)
            // This ensures that earlier edits don't affect the positions of later edits
            let original_content = content;
            let mut new_content_parts = Vec::new();
            let mut last_end = original_content.len();

            // Process edits in reverse order by position
            for (position, deleted_length, inserted_text) in edit_infos.iter() {
                let start = *position;
                let end = start + deleted_length;

                // Validate that the edit is within bounds of the original content
                if start > original_content.len() || end > original_content.len() {
                    eprintln!(
                        "Warning: Edit range {}..{} is beyond original content length {}. Skipping edit.",
                        start, end, original_content.len()
                    );
                    continue;
                }

                // Add the content after this edit (from end of edit to last_end)
                if end < last_end {
                    new_content_parts.push(&original_content.as_bytes()[end..last_end]);
                }

                // Add the replacement text
                new_content_parts.push(inserted_text);

                last_end = start;
            }

            // Add the content before the first edit (from 0 to last_end)
            if last_end > 0 {
                new_content_parts.push(&original_content.as_bytes()[0..last_end]);
            }

            // Reverse the parts since we built them in reverse order
            new_content_parts.reverse();

            // Concatenate all parts
            let mut result_bytes = Vec::new();
            for part in new_content_parts {
                result_bytes.extend_from_slice(part);
            }

            new_content = Cow::Owned(String::from_utf8(result_bytes).map_err(|e| {
                AstGrepError::Config(format!("Invalid UTF-8 after applying fixes: {e}"))
            })?);
        }
    }

    // Handle regular matches (rules without fixers or when not applying fixes)
    for (rule, rule_matches) in scan_result.matches.into_iter() {
        for match_item in rule_matches {
            let node = match_item.get_node();
            let start_pos = node.start_pos();
            let end_pos = node.end_pos();

            matches.push(AstGrepMatch {
                file_path: file_path.clone(),
                start_byte: node.range().start,
                end_byte: node.range().end,
                start_line: start_pos.line(),
                start_column: start_pos.column(node),
                end_line: end_pos.line(),
                end_column: end_pos.column(node),
                match_text: node.text().to_string(),
                rule_id: rule.id.clone(),
            });
        }
    }

    Ok(ScanResult {
        matches,
        file_modified,
        new_content,
    })
}
