use codemod_ast_grep_dynamic_lang::{DynamicLang, Registration};
use dirs::data_local_dir;
use reqwest;
use reqwest::header::CONTENT_LENGTH;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::{collections::HashSet, env, fmt, path::PathBuf, str::FromStr};
use tokio::io::AsyncWriteExt;
use tokio_stream::StreamExt;

use crate::sandbox::engine::language_data::get_extensions_for_language;

#[derive(PartialEq, Eq, Hash, Clone)]
struct ReadyLang {
    language: SupportedLanguage,
    extensions: Vec<String>,
    lib_path: PathBuf,
}

pub const BASE_URL: &str = env!("TREE_SITTER_BASE_URL");

pub async fn load_tree_sitter(
    languages: &[SupportedLanguage],
    progress_callback: Option<Arc<Box<dyn Fn(u64, u64) + Send + Sync>>>,
) -> Result<Vec<DynamicLang>, String> {
    let mut ready_langs = HashSet::new();
    for language in languages {
        let extensions = get_extensions_for_language(language.to_string().as_str());
        let os: &'static str = if env::consts::OS == "macos" {
            "darwin"
        } else if env::consts::OS == "windows" {
            "win32"
        } else if env::consts::OS == "linux" {
            "linux"
        } else {
            env::consts::OS
        };
        let arch = if env::consts::ARCH == "aarch64" {
            "arm64"
        } else if env::consts::ARCH == "x86_64" {
            "x64"
        } else {
            env::consts::ARCH
        };
        let extension = if os == "darwin" {
            "dylib"
        } else if os == "linux" {
            "so"
        } else if os == "win32" {
            "dll"
        } else {
            "so"
        };
        let lib_path = data_local_dir().unwrap().join(format!(
            "codemod/tree_sitter/{language}/{os}-{arch}.{extension}"
        ));
        if !lib_path.exists() {
            if let Some(parent) = lib_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create directory: {e}"))?;
            }
            let url = format!(
                "{BASE_URL}/tree-sitter/parsers/tree-sitter-{language}/latest/{os}-{arch}.{extension}"
            );

            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

            let head_response = client
                .head(&url)
                .send()
                .await
                .map_err(|e| format!("Failed to get header from {url}: {e}"))?;

            let total_size = head_response
                .headers()
                .get(CONTENT_LENGTH)
                .and_then(|val| val.to_str().ok()?.parse().ok())
                .unwrap_or(0);

            let response = client
                .get(&url)
                .send()
                .await
                .map_err(|e| format!("Failed to download from {url}: {e}"))?;

            let mut stream = response.bytes_stream();
            let mut file = tokio::fs::File::create(&lib_path)
                .await
                .map_err(|e| format!("Failed to create file: {e}"))?;

            let mut downloaded = 0u64;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| format!("Stream error from {url}: {e}"))?;
                file.write_all(&chunk)
                    .await
                    .map_err(|e| format!("Write error: {e}"))?;
                downloaded += chunk.len() as u64;
                if let Some(ref callback) = progress_callback {
                    callback(downloaded, total_size);
                }
            }

            file.flush()
                .await
                .map_err(|e| format!("Flush error: {e}"))?;
        }
        ready_langs.insert(ReadyLang {
            language: *language,
            extensions: extensions.iter().map(|s| s.to_string()).collect(),
            lib_path: lib_path.clone(),
        });
    }
    let registrations: Vec<Registration> = ready_langs
        .iter()
        .map(|lang| Registration {
            lang_name: lang.language.to_string(),
            lib_path: lang.lib_path.clone(),
            symbol: format!("tree_sitter_{}", lang.language),
            meta_var_char: Some('$'),
            expando_char: Some('$'),
            extensions: lang.extensions.iter().map(|s| s.to_string()).collect(),
        })
        .collect();

    DynamicLang::register(registrations)
        .map_err(|e| format!("Failed to register tree-sitter languages: {e}"))?;
    Ok(ready_langs
        .into_iter()
        .map(|lang| DynamicLang::from_str(&lang.language.to_string()).unwrap())
        .collect())
}

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
        };
        write!(f, "{name}")
    }
}

impl FromStr for SupportedLanguage {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "typescript" => Ok(SupportedLanguage::Typescript),
            "javascript" => Ok(SupportedLanguage::Javascript),
            "python" => Ok(SupportedLanguage::Python),
            "rust" => Ok(SupportedLanguage::Rust),
            "go" => Ok(SupportedLanguage::Go),
            "java" => Ok(SupportedLanguage::Java),
            "tsx" => Ok(SupportedLanguage::Tsx),
            "css" => Ok(SupportedLanguage::Css),
            "html" => Ok(SupportedLanguage::Html),
            "kotlin" => Ok(SupportedLanguage::Kotlin),
            _ => Err(format!("Unsupported language: {s}")),
        }
    }
}
