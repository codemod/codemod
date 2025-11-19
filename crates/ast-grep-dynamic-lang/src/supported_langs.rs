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
            _ => Err(format!("Unsupported language: {s}")),
        }
    }
}
