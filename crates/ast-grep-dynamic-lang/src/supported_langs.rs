use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum SupportedLanguage {
    Typescript,
    Javascript,
    Python,
    Rust,
    Go,
    Java,
    Tsx,
    Css,
    Html,
    Kotlin,
    Angular,
    Csharp,
    Cpp,
    C,
    Php,
    Ruby,
    Elixir,
    Json,
    Yaml,
    Bash,
    Haskell,
    Lua,
    Scala,
    Swift,
}

impl SupportedLanguage {
    pub fn all_langs() -> Vec<Self> {
        vec![
            Self::Typescript,
            Self::Javascript,
            Self::Python,
            Self::Rust,
            Self::Go,
            Self::Java,
            Self::Tsx,
            Self::Css,
            Self::Html,
            Self::Kotlin,
            Self::Angular,
            Self::Csharp,
            Self::Cpp,
            Self::C,
            Self::Php,
            Self::Ruby,
            Self::Elixir,
            Self::Json,
            Self::Yaml,
            Self::Bash,
            Self::Haskell,
            Self::Lua,
            Self::Scala,
            Self::Swift,
        ]
    }
}

impl fmt::Display for SupportedLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            SupportedLanguage::Typescript => "typescript",
            SupportedLanguage::Javascript => "javascript",
            SupportedLanguage::Python => "python",
            SupportedLanguage::Rust => "rust",
            SupportedLanguage::Go => "go",
            SupportedLanguage::Java => "java",
            SupportedLanguage::Tsx => "tsx",
            SupportedLanguage::Css => "css",
            SupportedLanguage::Html => "html",
            SupportedLanguage::Kotlin => "kotlin",
            SupportedLanguage::Angular => "angular",
            SupportedLanguage::Csharp => "c-sharp",
            SupportedLanguage::Cpp => "cpp",
            SupportedLanguage::C => "c",
            SupportedLanguage::Php => "php",
            SupportedLanguage::Ruby => "ruby",
            SupportedLanguage::Elixir => "elixir",
            SupportedLanguage::Json => "json",
            SupportedLanguage::Yaml => "yaml",
            SupportedLanguage::Bash => "bash",
            SupportedLanguage::Haskell => "haskell",
            SupportedLanguage::Lua => "lua",
            SupportedLanguage::Scala => "scala",
            SupportedLanguage::Swift => "swift",
        };
        write!(f, "{name}")
    }
}

impl FromStr for SupportedLanguage {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "typescript" => Ok(SupportedLanguage::Typescript),
            "ts" => Ok(SupportedLanguage::Typescript),
            "javascript" => Ok(SupportedLanguage::Javascript),
            "js" => Ok(SupportedLanguage::Javascript),
            "jsx" => Ok(SupportedLanguage::Javascript),
            "python" => Ok(SupportedLanguage::Python),
            "py" => Ok(SupportedLanguage::Python),
            "rust" => Ok(SupportedLanguage::Rust),
            "rs" => Ok(SupportedLanguage::Rust),
            "go" => Ok(SupportedLanguage::Go),
            "golang" => Ok(SupportedLanguage::Go),
            "java" => Ok(SupportedLanguage::Java),
            "tsx" => Ok(SupportedLanguage::Tsx),
            "css" => Ok(SupportedLanguage::Css),
            "html" => Ok(SupportedLanguage::Html),
            "kotlin" => Ok(SupportedLanguage::Kotlin),
            "kt" => Ok(SupportedLanguage::Kotlin),
            "angular" => Ok(SupportedLanguage::Angular),
            "csharp" => Ok(SupportedLanguage::Csharp),
            "c-sharp" => Ok(SupportedLanguage::Csharp),
            "cs" => Ok(SupportedLanguage::Csharp),
            "c#" => Ok(SupportedLanguage::Csharp),
            "cpp" => Ok(SupportedLanguage::Cpp),
            "c++" => Ok(SupportedLanguage::Cpp),
            "cc" => Ok(SupportedLanguage::Cpp),
            "cxx" => Ok(SupportedLanguage::Cpp),
            "c" => Ok(SupportedLanguage::C),
            "php" => Ok(SupportedLanguage::Php),
            "ruby" => Ok(SupportedLanguage::Ruby),
            "rb" => Ok(SupportedLanguage::Ruby),
            "elixir" => Ok(SupportedLanguage::Elixir),
            "ex" => Ok(SupportedLanguage::Elixir),
            "json" => Ok(SupportedLanguage::Json),
            "yaml" => Ok(SupportedLanguage::Yaml),
            "bash" => Ok(SupportedLanguage::Bash),
            "haskell" => Ok(SupportedLanguage::Haskell),
            "lua" => Ok(SupportedLanguage::Lua),
            "scala" => Ok(SupportedLanguage::Scala),
            "swift" => Ok(SupportedLanguage::Swift),
            _ => Err(format!("Unsupported language: {s}")),
        }
    }
}

/// Creates a map from SupportLang to their associated file extensions
pub fn create_language_extension_map() -> HashMap<SupportedLanguage, Vec<&'static str>> {
    let mut map = HashMap::new();

    map.insert(
        SupportedLanguage::Javascript,
        vec![".js", ".mjs", ".cjs", ".jsx"],
    );
    map.insert(
        SupportedLanguage::Typescript,
        vec![".ts", ".mts", ".cts", ".js", ".mjs", ".cjs"],
    );
    map.insert(
        SupportedLanguage::Tsx,
        vec![".tsx", ".jsx", ".ts", ".js", ".mjs", ".cjs", ".mts", ".cts"],
    );
    map.insert(
        SupportedLanguage::Bash,
        vec![".sh", ".bash", ".zsh", ".fish"],
    );
    map.insert(SupportedLanguage::C, vec![".c", ".h"]);
    map.insert(SupportedLanguage::Csharp, vec![".cs"]);
    map.insert(SupportedLanguage::Csharp, vec![".cs"]);
    map.insert(SupportedLanguage::Css, vec![".css"]);
    map.insert(
        SupportedLanguage::Cpp,
        vec![".cpp", ".cxx", ".cc", ".c++", ".hpp", ".hxx", ".hh", ".h++"],
    );
    map.insert(SupportedLanguage::Elixir, vec![".ex", ".exs"]);
    map.insert(SupportedLanguage::Go, vec![".go"]);
    map.insert(SupportedLanguage::Haskell, vec![".hs", ".lhs"]);
    map.insert(SupportedLanguage::Html, vec![".html", ".htm"]);
    map.insert(SupportedLanguage::Java, vec![".java"]);
    map.insert(SupportedLanguage::Json, vec![".json", ".jsonc"]);
    map.insert(SupportedLanguage::Kotlin, vec![".kt", ".kts"]);
    map.insert(SupportedLanguage::Lua, vec![".lua"]);
    map.insert(
        SupportedLanguage::Php,
        vec![
            ".php", ".phtml", ".php3", ".php4", ".php5", ".php7", ".phps", ".php-s",
        ],
    );
    map.insert(SupportedLanguage::Python, vec![".py", ".pyw", ".pyi"]);
    map.insert(SupportedLanguage::Ruby, vec![".rb", ".rbw"]);
    map.insert(SupportedLanguage::Rust, vec![".rs"]);
    map.insert(SupportedLanguage::Scala, vec![".scala", ".sc"]);
    map.insert(SupportedLanguage::Swift, vec![".swift"]);
    map.insert(SupportedLanguage::Yaml, vec![".yaml", ".yml"]);

    map
}

/// Get file extensions for a specific language
pub fn get_extensions_for_language(lang: SupportedLanguage) -> Vec<&'static str> {
    let map = create_language_extension_map();
    map.get(&lang).cloned().unwrap_or_default()
}

/// Determine language from file extension
#[allow(dead_code)]
pub fn get_language_from_extension(extension: &str) -> Option<SupportedLanguage> {
    let map = create_language_extension_map();

    for (lang, extensions) in map.iter() {
        if extensions.contains(&extension) {
            return Some(*lang);
        }
    }

    None
}

/// Get all supported file extensions
#[allow(dead_code)]
pub fn get_all_supported_extensions() -> Vec<&'static str> {
    let map = create_language_extension_map();
    let mut extensions: Vec<&'static str> = map.values().flatten().copied().collect();
    extensions.sort();
    extensions.dedup();
    extensions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_extension_mapping() {
        let map = create_language_extension_map();
        assert!(!map.is_empty());

        assert!(map
            .get(&SupportedLanguage::Javascript)
            .unwrap()
            .contains(&".js"));
        assert!(map
            .get(&SupportedLanguage::Typescript)
            .unwrap()
            .contains(&".ts"));
        assert!(map.get(&SupportedLanguage::Rust).unwrap().contains(&".rs"));
    }

    #[test]
    fn test_get_extensions_for_language() {
        let js_extensions = get_extensions_for_language(SupportedLanguage::Javascript);
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
