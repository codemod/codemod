use std::collections::HashMap;

/// Creates a map from SupportLang to their associated file extensions
pub fn create_language_extension_map() -> HashMap<&'static str, Vec<&'static str>> {
    let mut map = HashMap::new();

    #[cfg(feature = "native")]
    {
        map.insert("javascript", vec![".js", ".mjs", ".cjs", ".jsx"]);
        map.insert(
            "typescript",
            vec![".ts", ".mts", ".cts", ".js", ".mjs", ".cjs"],
        );
        map.insert(
            "tsx",
            vec![".tsx", ".jsx", ".ts", ".js", ".mjs", ".cjs", ".mts", ".cts"],
        );
        map.insert("bash", vec![".sh", ".bash", ".zsh", ".fish"]);
        map.insert("c", vec![".c", ".h"]);
        map.insert("csharp", vec![".cs"]);
        map.insert("css", vec![".css"]);
        map.insert(
            "cpp",
            vec![".cpp", ".cxx", ".cc", ".c++", ".hpp", ".hxx", ".hh", ".h++"],
        );
        map.insert("elixir", vec![".ex", ".exs"]);
        map.insert("go", vec![".go"]);
        map.insert("haskell", vec![".hs", ".lhs"]);
        map.insert("html", vec![".html", ".htm"]);
        map.insert("java", vec![".java"]);
        map.insert("json", vec![".json", ".jsonc"]);
        map.insert("kotlin", vec![".kt", ".kts"]);
        map.insert("lua", vec![".lua"]);
        map.insert(
            "php",
            vec![
                ".php", ".phtml", ".php3", ".php4", ".php5", ".php7", ".phps", ".php-s",
            ],
        );
        map.insert("python", vec![".py", ".pyw", ".pyi"]);
        map.insert("ruby", vec![".rb", ".rbw"]);
        map.insert("rust", vec![".rs"]);
        map.insert("scala", vec![".scala", ".sc"]);
        map.insert("swift", vec![".swift"]);
        map.insert("yaml", vec![".yaml", ".yml"]);
    }

    map
}

/// Get file extensions for a specific language
pub fn get_extensions_for_language(lang: &str) -> Vec<&'static str> {
    let map = create_language_extension_map();
    map.get(&lang).cloned().unwrap_or_default()
}

/// Determine language from file extension
pub fn get_language_from_extension(extension: &str) -> Option<&'static str> {
    let map = create_language_extension_map();

    for (lang, extensions) in map.iter() {
        if extensions.contains(&extension) {
            return Some(*lang);
        }
    }

    None
}

/// Get all supported file extensions
pub fn get_all_supported_extensions() -> Vec<&'static str> {
    let map = create_language_extension_map();
    let mut extensions: Vec<&'static str> = map.values().flatten().copied().collect();
    extensions.sort();
    extensions.dedup();
    extensions
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;

    #[test]
    fn test_language_extension_mapping() {
        let map = create_language_extension_map();
        assert!(!map.is_empty());

        #[cfg(feature = "native")]
        {
            assert!(map.get(&"javascript").unwrap().contains(&".js"));
            assert!(map.get(&"typescript").unwrap().contains(&".ts"));
            assert!(map.get(&"rust").unwrap().contains(&".rs"));
        }
    }

    #[test]
    fn test_get_extensions_for_language() {
        let js_extensions = get_extensions_for_language("javascript");
        assert!(js_extensions.contains(&".js"));
        assert!(js_extensions.contains(&".mjs"));
        assert!(js_extensions.contains(&".cjs"));
    }

    #[test]
    fn test_get_language_from_extension() {
        let lang = get_language_from_extension(".rs");
        assert!(lang.is_some());

        let lang = get_language_from_extension(".unknown");
        assert!(lang.is_none());
    }

    #[test]
    fn test_get_all_supported_extensions() {
        let extensions = get_all_supported_extensions();
        assert!(!extensions.is_empty());
        assert!(extensions.contains(&".js"));
        assert!(extensions.contains(&".rs"));
        assert!(extensions.contains(&".py"));
    }
}
