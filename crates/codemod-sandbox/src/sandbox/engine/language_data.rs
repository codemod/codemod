use std::collections::HashMap;

#[cfg(feature = "native")]
use super::codemod_lang::CodemodLang;

/// Creates a map from CodemodLang to their associated file extensions
pub fn create_language_extension_map() -> HashMap<CodemodLang, Vec<&'static str>> {
    let mut map = HashMap::new();

    #[cfg(feature = "native")]
    {
        use ast_grep_language::SupportLang::*;

        map.insert(
            CodemodLang::Static(JavaScript),
            vec![".js", ".mjs", ".cjs", ".jsx"],
        );
        map.insert(
            CodemodLang::Static(TypeScript),
            vec![".ts", ".mts", ".cts", ".js", ".mjs", ".cjs"],
        );
        map.insert(
            CodemodLang::Static(Tsx),
            vec![".tsx", ".jsx", ".ts", ".js", ".mjs", ".cjs", ".mts", ".cts"],
        );
        map.insert(
            CodemodLang::Static(Bash),
            vec![".sh", ".bash", ".zsh", ".fish"],
        );
        map.insert(CodemodLang::Static(C), vec![".c", ".h"]);
        map.insert(CodemodLang::Static(CSharp), vec![".cs"]);
        map.insert(CodemodLang::Static(Css), vec![".css"]);
        map.insert(
            CodemodLang::Static(Cpp),
            vec![".cpp", ".cxx", ".cc", ".c++", ".hpp", ".hxx", ".hh", ".h++"],
        );
        map.insert(CodemodLang::Static(Elixir), vec![".ex", ".exs"]);
        map.insert(CodemodLang::Static(Go), vec![".go"]);
        map.insert(CodemodLang::Static(Haskell), vec![".hs", ".lhs"]);
        map.insert(CodemodLang::Static(Html), vec![".html", ".htm"]);
        map.insert(CodemodLang::Static(Java), vec![".java"]);
        map.insert(CodemodLang::Static(Json), vec![".json", ".jsonc"]);
        map.insert(CodemodLang::Static(Kotlin), vec![".kt", ".kts"]);
        map.insert(CodemodLang::Static(Lua), vec![".lua"]);
        map.insert(
            CodemodLang::Static(Php),
            vec![
                ".php", ".phtml", ".php3", ".php4", ".php5", ".php7", ".phps", ".php-s",
            ],
        );
        map.insert(CodemodLang::Static(Python), vec![".py", ".pyw", ".pyi"]);
        map.insert(CodemodLang::Static(Ruby), vec![".rb", ".rbw"]);
        map.insert(CodemodLang::Static(Rust), vec![".rs"]);
        map.insert(CodemodLang::Static(Scala), vec![".scala", ".sc"]);
        map.insert(CodemodLang::Static(Swift), vec![".swift"]);
        map.insert(CodemodLang::Static(Yaml), vec![".yaml", ".yml"]);

        // Dynamic languages (registered via tree-sitter-loader)
        if let Ok(lang) = std::str::FromStr::from_str("less") {
            let lang: CodemodLang = lang;
            map.insert(lang, vec![".less"]);
        }
    }

    map
}

/// Get file extensions for a specific language
pub fn get_extensions_for_language(lang: CodemodLang) -> Vec<&'static str> {
    let map = create_language_extension_map();
    map.get(&lang).cloned().unwrap_or_default()
}

/// Determine language from file extension
pub fn get_language_from_extension(extension: &str) -> Option<CodemodLang> {
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
    use ast_grep_language::SupportLang;

    #[test]
    fn test_language_extension_mapping() {
        let map = create_language_extension_map();
        assert!(!map.is_empty());

        assert!(map
            .get(&CodemodLang::Static(SupportLang::JavaScript))
            .unwrap()
            .contains(&".js"));
        assert!(map
            .get(&CodemodLang::Static(SupportLang::TypeScript))
            .unwrap()
            .contains(&".ts"));
        assert!(map
            .get(&CodemodLang::Static(SupportLang::Rust))
            .unwrap()
            .contains(&".rs"));
    }

    #[test]
    fn test_get_extensions_for_language() {
        let js_extensions =
            get_extensions_for_language(CodemodLang::Static(SupportLang::JavaScript));
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
