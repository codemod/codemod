use std::panic;

use ast_grep_config::CombinedScan;
use ast_grep_core::{replacer::Content, AstGrep, Doc, Language};

use crate::ast_grep::types::{AstGrepError, AstGrepMatch};

pub(crate) struct ScanResult {
    pub matches: Vec<AstGrepMatch>,
    pub file_modified: bool,
    pub new_content: String,
}

pub(crate) fn scan_content<D: Doc<Lang = L>, L: Language>(
    root: &AstGrep<D>,
    content: &str,
    file_path: String,
    combined_scan: &CombinedScan<L>,
    apply_fixes: bool,
) -> Result<ScanResult, AstGrepError>
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
    let mut file_modified = false;
    let mut new_content = content.to_string();

    // Handle diffs (rules with fixers) when applying fixes
    if apply_fixes && !scan_result.diffs.is_empty() {
        // (position, deleted_length, inserted_text), in the scan's order:
        // depth first, so an outer node comes before the nodes inside it.
        let mut edits: Vec<(usize, usize, Vec<u8>)> = Vec::new();

        for (rule, node_match) in &scan_result.diffs {
            if let Ok(fixers) = rule.get_fixer() {
                if let Some(fixer) = fixers.first() {
                    let edit = node_match.make_edit(&rule.matcher, fixer);
                    edits.push((edit.position, edit.deleted_length, edit.inserted_text));
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

        if !edits.is_empty() {
            new_content = apply_edits(content, edits)?;
            file_modified = true;
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

/// Apply `(position, deleted_length, inserted_text)` edits to `content`, as
/// `ast-grep --update-all` does: in order of position, and an edit that
/// starts inside one already applied is dropped. Edits come in the scan's
/// order, outer nodes first, and the sort keeps that order for edits that
/// start at the same byte, so of two nested matches the outer one wins.
fn apply_edits(
    content: &str,
    mut edits: Vec<(usize, usize, Vec<u8>)>,
) -> Result<String, AstGrepError> {
    edits.sort_by_key(|(position, _, _)| *position);
    let bytes = content.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut done = 0;
    for (start, deleted, inserted) in edits {
        let end = start + deleted;
        if start < done || end > bytes.len() {
            continue;
        }
        result.extend_from_slice(&bytes[done..start]);
        result.extend_from_slice(&inserted);
        done = end;
    }
    result.extend_from_slice(&bytes[done..]);
    String::from_utf8(result)
        .map_err(|e| AstGrepError::Config(format!("Invalid UTF-8 after applying fixes: {e}")))
}

#[cfg(test)]
mod tests {
    use super::apply_edits;

    fn edit(position: usize, deleted: usize, text: &str) -> (usize, usize, Vec<u8>) {
        (position, deleted, text.as_bytes().to_vec())
    }

    #[test]
    fn an_edit_inside_an_earlier_one_is_dropped() {
        // `f(g(x))`: the outer call (0..7) and the inner one (2..6).
        let edits = vec![edit(0, 7, "F(g(x))"), edit(2, 4, "G(x)")];
        assert_eq!(apply_edits("f(g(x));", edits).unwrap(), "F(g(x));");
    }

    #[test]
    fn of_two_edits_at_one_position_the_first_wins() {
        let edits = vec![edit(0, 5, "outer"), edit(0, 1, "i")];
        assert_eq!(apply_edits("a.b.c", edits).unwrap(), "outer");
    }

    #[test]
    fn separate_edits_all_apply_whatever_their_order() {
        let edits = vec![
            edit(8, 1, "Z"),
            edit(0, 1, "X"),
            edit(4, 1, "Y"),
            edit(9, 0, "!"),
        ];
        assert_eq!(apply_edits("a = b + c;", edits).unwrap(), "X = Y + Z!;");
    }
}
