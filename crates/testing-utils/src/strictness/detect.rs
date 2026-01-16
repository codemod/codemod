//! Language detection from file paths.

use std::path::Path;

use super::compare::loose_compare_with_registry;
use super::registry::NormalizerRegistry;

/// Detect language from file extension using a custom registry.
pub fn detect_language_from_path(
    path: &Path,
    registry: &NormalizerRegistry,
) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?;
    let ext_with_dot = format!(".{}", ext.to_lowercase());
    registry
        .get_by_extension(&ext_with_dot)
        .and_then(|n| n.language_ids().first().copied())
}

/// Detect language from file extension using the default registry.
pub fn detect_language(path: &Path) -> Option<&'static str> {
    detect_language_from_path(path, NormalizerRegistry::default_ref())
}

/// Compare two code strings with automatic language detection from file path.
///
/// Falls back to `fallback_language` if provided, otherwise exact string comparison.
pub fn loose_compare_with_path(
    expected: &str,
    actual: &str,
    file_path: &Path,
    fallback_language: Option<&str>,
) -> bool {
    loose_compare_with_path_and_registry(
        expected,
        actual,
        file_path,
        fallback_language,
        NormalizerRegistry::default_ref(),
    )
}

/// Compare with automatic language detection using a custom registry.
pub fn loose_compare_with_path_and_registry(
    expected: &str,
    actual: &str,
    file_path: &Path,
    fallback_language: Option<&str>,
    registry: &NormalizerRegistry,
) -> bool {
    match detect_language_from_path(file_path, registry).or(fallback_language) {
        Some(lang) => loose_compare_with_registry(expected, actual, lang, registry),
        None => expected == actual,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_detect_language_javascript() {
        assert_eq!(detect_language(Path::new("file.js")), Some("javascript"));
        assert_eq!(detect_language(Path::new("file.jsx")), Some("javascript"));
        assert_eq!(detect_language(Path::new("file.mjs")), Some("javascript"));
        assert_eq!(detect_language(Path::new("file.cjs")), Some("javascript"));
    }

    #[test]
    fn test_detect_language_typescript() {
        assert_eq!(detect_language(Path::new("file.ts")), Some("typescript"));
        assert_eq!(detect_language(Path::new("file.mts")), Some("typescript"));
        assert_eq!(detect_language(Path::new("file.tsx")), Some("tsx"));
    }

    #[test]
    fn test_detect_language_python() {
        assert_eq!(detect_language(Path::new("file.py")), Some("python"));
        assert_eq!(detect_language(Path::new("file.pyi")), Some("python"));
        assert_eq!(detect_language(Path::new("file.pyw")), Some("python"));
    }

    #[test]
    fn test_detect_language_others() {
        assert_eq!(detect_language(Path::new("file.go")), Some("go"));
        assert_eq!(detect_language(Path::new("file.rs")), Some("rust"));
        assert_eq!(detect_language(Path::new("file.json")), Some("json"));
    }

    #[test]
    fn test_detect_language_unknown() {
        assert_eq!(detect_language(Path::new("file.css")), None);
        assert_eq!(detect_language(Path::new("file.html")), None);
        assert_eq!(detect_language(Path::new("file")), None);
    }

    #[test]
    fn test_detect_language_case_insensitive() {
        assert_eq!(detect_language(Path::new("file.JS")), Some("javascript"));
        assert_eq!(detect_language(Path::new("file.PY")), Some("python"));
    }

    #[test]
    fn test_loose_compare_with_path_js() {
        let expected = r#"const obj = { a: 1, b: 2 };"#;
        let actual = r#"const obj = { b: 2, a: 1 };"#;
        let path = PathBuf::from("test.js");

        assert!(loose_compare_with_path(expected, actual, &path, None));
    }

    #[test]
    fn test_loose_compare_with_path_python() {
        let expected = "func(a=1, b=2)";
        let actual = "func(b=2, a=1)";
        let path = PathBuf::from("test.py");

        assert!(loose_compare_with_path(expected, actual, &path, None));
    }

    #[test]
    fn test_loose_compare_with_path_fallback() {
        let path = PathBuf::from("test.unknown");
        assert!(loose_compare_with_path(
            "func(a=1, b=2)",
            "func(b=2, a=1)",
            &path,
            Some("python")
        ));
    }

    #[test]
    fn test_loose_compare_with_path_no_fallback() {
        let path = PathBuf::from("test.unknown");
        assert!(loose_compare_with_path(
            "some code",
            "some code",
            &path,
            None
        ));
        assert!(!loose_compare_with_path(
            "some code",
            "different code",
            &path,
            None
        ));
    }
}
