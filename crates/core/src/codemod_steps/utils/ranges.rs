use std::ops::Range;

pub(crate) fn quoted_string_content_range(
    content: &str,
    literal_range: Range<usize>,
) -> Option<(String, Range<usize>)> {
    let literal = content.get(literal_range.clone())?;
    let first_quote_offset = literal.find(['"', '\''])?;
    let quote = literal[first_quote_offset..].chars().next()?;
    let content_start = literal_range.start + first_quote_offset + quote.len_utf8();
    let content_end =
        content_start + literal[first_quote_offset + quote.len_utf8()..].find(quote)?;
    Some((
        content.get(content_start..content_end)?.to_string(),
        content_start..content_end,
    ))
}

pub(crate) fn trim_range(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    let leading = text.len() - text.trim_start().len();
    let trailing = text.len() - text.trim_end().len();
    let start = range.start + leading;
    let end = range.end.checked_sub(trailing)?;
    (start <= end).then_some(start..end)
}

pub(crate) fn content_slice(content: &str, range: Range<usize>) -> Option<&str> {
    content.get(range)
}

pub(crate) fn replace_range(
    content: &str,
    range: Range<usize>,
    replacement: &str,
) -> Result<String, String> {
    if !content.is_char_boundary(range.start) || !content.is_char_boundary(range.end) {
        return Err("ast-grep produced a non-character-boundary edit range".to_string());
    }
    let mut updated = content.to_string();
    updated.replace_range(range, replacement);
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_quoted_string_content_range() {
        let content = r#"implementation("org.slf4j:slf4j-api:2.0.9")"#;
        let literal_start = content.find('"').unwrap();
        let literal_end = content.rfind('"').unwrap() + 1;

        let (value, range) = quoted_string_content_range(content, literal_start..literal_end)
            .expect("quoted string should be parsed");

        assert_eq!(value, "org.slf4j:slf4j-api:2.0.9");
        assert_eq!(&content[range], "org.slf4j:slf4j-api:2.0.9");
    }

    #[test]
    fn extracts_single_quoted_string_content_range() {
        let content = "implementation 'org.slf4j:slf4j-api:2.0.9'";
        let literal_start = content.find('\'').unwrap();
        let literal_end = content.rfind('\'').unwrap() + 1;

        let (value, range) = quoted_string_content_range(content, literal_start..literal_end)
            .expect("quoted string should be parsed");

        assert_eq!(value, "org.slf4j:slf4j-api:2.0.9");
        assert_eq!(&content[range], "org.slf4j:slf4j-api:2.0.9");
    }

    #[test]
    fn trims_range_with_surrounding_whitespace() {
        let content = "  1.2.3\n";
        let range = trim_range(content, 0..content.len()).unwrap();

        assert_eq!(&content[range], "1.2.3");
    }

    #[test]
    fn replaces_character_boundary_range() {
        let content = "version = 1.0.0";
        let updated = replace_range(content, 10..15, "2.0.0").unwrap();

        assert_eq!(updated, "version = 2.0.0");
    }

    #[test]
    fn rejects_non_character_boundary_range() {
        let content = "version = é";
        let start = content.find('é').unwrap() + 1;
        let error = replace_range(content, start..content.len(), "e").unwrap_err();

        assert!(error.contains("non-character-boundary"));
    }

    #[test]
    fn slices_content_by_range() {
        let content = "abc";

        assert_eq!(content_slice(content, 1..3), Some("bc"));
        assert_eq!(content_slice(content, 1..4), None);
    }
}
