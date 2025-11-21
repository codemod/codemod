use crate::download_utils::{download_file, get_system_info, ProgressCallback};
use crate::supported_langs::{get_extensions_for_language, SupportedLanguage};
use crate::{DynamicLang, Registration};
use dirs::data_local_dir;
use std::{collections::HashSet, path::PathBuf, str::FromStr};

#[derive(PartialEq, Eq, Hash, Clone)]
struct ReadyLang {
    language: SupportedLanguage,
    extensions: Vec<String>,
    lib_path: PathBuf,
}

pub fn base_url() -> String {
    std::env::var("TREE_SITTER_BASE_URL")
        .unwrap_or_else(|_| "https://tree-sitter-parsers.s3.us-east-1.amazonaws.com".to_string())
}

pub use crate::download_utils::{
    download_file as download_file_reexport, get_system_info as get_system_info_reexport,
};

pub async fn load_tree_sitter(
    languages: &[SupportedLanguage],
    progress_callback: Option<ProgressCallback>,
) -> Result<Vec<DynamicLang>, String> {
    let mut ready_langs = HashSet::new();
    let languages_to_download = languages
        .iter()
        .filter(|l| {
            !DynamicLang::all_langs()
                .iter()
                .any(|d| d.name().to_lowercase() == l.to_string().as_str().to_lowercase())
        })
        .copied()
        .collect::<Vec<SupportedLanguage>>();
    let (os, arch, extension) = get_system_info();
    for language in languages_to_download.as_slice() {
        let extensions = get_extensions_for_language(*language);
        let lib_path = data_local_dir().unwrap().join(format!(
            "codemod/tree_sitter/{language}/{os}-{arch}.{extension}"
        ));
        if !lib_path.exists() {
            let url = format!(
                "{}/tree-sitter/parsers/tree-sitter-{language}/latest/{os}-{arch}.{extension}",
                base_url()
            );
            download_file(&url, &lib_path, progress_callback.clone()).await?;
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
            symbol: format!(
                "tree_sitter_{}",
                lang.language.to_string().replace("-", "_")
            ),
            meta_var_char: Some('$'),
            expando_char: Some('$'),
            extensions: lang.extensions.iter().map(|s| s.to_string()).collect(),
        })
        .collect();

    if !ready_langs.is_empty() {
        DynamicLang::register(registrations)
            .map_err(|e| format!("Failed to register tree-sitter languages: {e}"))?;
    }
    Ok(languages
        .iter()
        .filter_map(|lang| DynamicLang::from_str(&lang.to_string()).ok())
        .collect())
}
