//! How hints tell someone to run this CLI again.
//!
//! Hints pin the running version (`npx codemod@2.0.0-alpha.7 login`)
//! instead of the `latest` tag: while 2.x is a prerelease, `latest` is 1.x,
//! a different CLI. The MCP launcher `codemod ai` writes is pinned the same
//! way.

use crate::CLI_VERSION;

/// `codemod@<running version>`, the npm package spec of this CLI.
pub(crate) fn codemod_package() -> String {
    format!("codemod@{CLI_VERSION}")
}

/// `npx codemod@<running version> <args>`.
pub(crate) fn npx_codemod(args: &str) -> String {
    format!("npx {} {args}", codemod_package())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_run_this_version_not_latest() {
        assert_eq!(
            npx_codemod("login"),
            format!("npx codemod@{CLI_VERSION} login")
        );
        assert!(!npx_codemod("login").contains("@latest"));
    }

    /// `text` with every `#[cfg(test)]` item (a test module, or one test-only
    /// function or constant) blanked out, line breaks kept so line numbers
    /// still match. Braces in strings, chars and comments are not counted.
    fn without_test_items(text: &str) -> String {
        let bytes = text.as_bytes();
        let mut out = bytes.to_vec();
        let mut from = 0;
        while let Some(found) = text[from..].find("#[cfg(test)]") {
            let start = from + found;
            let end = item_end(bytes, start + "#[cfg(test)]".len());
            for byte in &mut out[start..end] {
                if *byte != b'\n' {
                    *byte = b' ';
                }
            }
            from = end;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// Where the item starting at `at` ends: after its `;`, or after the `}`
    /// matching its first `{`.
    fn item_end(bytes: &[u8], at: usize) -> usize {
        let mut depth = 0usize;
        let mut i = at;
        while i < bytes.len() {
            match bytes[i] {
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    while i < bytes.len() && bytes[i] != b'\n' {
                        i += 1;
                    }
                    continue;
                }
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    i += 2;
                    while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                        i += 1;
                    }
                    i += 2;
                    continue;
                }
                b'r' if matches!(bytes.get(i + 1), Some(b'"' | b'#'))
                    && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric()) =>
                {
                    // r"..." or r#"..."#
                    let hashes = bytes[i + 1..].iter().take_while(|b| **b == b'#').count();
                    if bytes.get(i + 1 + hashes) == Some(&b'"') {
                        let close: Vec<u8> = std::iter::once(b'"')
                            .chain(std::iter::repeat_n(b'#', hashes))
                            .collect();
                        i += 2 + hashes;
                        while i < bytes.len() && !bytes[i..].starts_with(&close) {
                            i += 1;
                        }
                        i += close.len();
                        continue;
                    }
                }
                b'"' => {
                    i += 1;
                    while i < bytes.len() && bytes[i] != b'"' {
                        i += if bytes[i] == b'\\' { 2 } else { 1 };
                    }
                }
                // A char literal ('{', '\''), not a lifetime ('a).
                b'\'' if bytes.get(i + 2) == Some(&b'\'') => i += 2,
                b'\'' if bytes.get(i + 1) == Some(&b'\\') => {
                    i += 2;
                    while i < bytes.len() && bytes[i] != b'\'' {
                        i += 1;
                    }
                }
                b'{' => depth += 1,
                b'}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return i + 1;
                    }
                }
                b';' if depth == 0 => return i + 1,
                _ => {}
            }
            i += 1;
        }
        bytes.len()
    }

    #[test]
    fn only_test_items_are_left_out() {
        let text = r##"const A: &str = "keep 1";
#[cfg(test)]
const B: &str = "drop 1";
#[cfg(test)]
fn helper() -> &'static str { let _ = '{'; "drop 2 }" }
fn code() { println!("keep 2 {}", "{"); } // }
#[cfg(test)]
mod tests {
    #[test]
    fn t() { let _ = r#"drop 3 "}" "#; /* } */ }
}
const C: &str = "keep 3";
"##;
        let kept = without_test_items(text);
        assert_eq!(kept.lines().count(), text.lines().count());
        for keep in ["keep 1", "keep 2", "keep 3"] {
            assert!(kept.contains(keep), "{keep} missing:\n{kept}");
        }
        for drop in ["drop 1", "drop 2", "drop 3"] {
            assert!(!kept.contains(drop), "{drop} kept:\n{kept}");
        }
        assert_eq!(kept.lines().nth(11), Some("const C: &str = \"keep 3\";"));
    }

    /// No hint in the CLI's own messages sends someone to `codemod@latest`.
    #[test]
    fn no_message_points_at_codemod_latest() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for entry in walkdir::WalkDir::new(&src) {
            let entry = entry.unwrap();
            if entry.path().extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(entry.path()).unwrap();
            // Test code may name `@latest`: test data, not a message.
            let code = without_test_items(&text);
            for (number, line) in code.lines().enumerate() {
                if line.contains("codemod@latest") {
                    offenders.push(format!("{}:{}", entry.path().display(), number + 1));
                }
            }
        }
        assert!(offenders.is_empty(), "{offenders:#?}");
    }
}
