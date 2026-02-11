use ast_grep_dynamic::{DynamicLang, Registration};
use std::path::{Path, PathBuf};
use std::sync::Once;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LoaderError {
    #[error("Failed to download parser: {0}")]
    Download(String),
    #[error("Failed to register parser: {0}")]
    Register(String),
    #[error("No cache directory available")]
    NoCacheDir,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Unsupported platform: os={os}, arch={arch}")]
    UnsupportedPlatform { os: String, arch: String },
}

struct DynamicLanguageDefinition {
    name: &'static str,
    symbol: &'static str,
    extensions: &'static [&'static str],
    expando_char: char,
    urls: &'static [(&'static str, &'static str, &'static str)], // (os, arch, url)
}

fn get_definitions() -> &'static [DynamicLanguageDefinition] {
    &[DynamicLanguageDefinition {
        name: "less",
        symbol: "tree_sitter_less",
        extensions: &["less"],
        expando_char: '_',
        urls: &[
            (
                "macos",
                "aarch64",
                concat!(
                    "https://tree-sitter-parsers.s3.us-east-1.amazonaws.com/tree-sitter/parsers/tree-sitter-less/",
                    "945f52c94250309073a96bbfbc5bcd57ff2bde49/darwin-arm64.dylib"
                ),
            ),
            (
                "macos",
                "x86_64",
                concat!(
                    "https://tree-sitter-parsers.s3.us-east-1.amazonaws.com/tree-sitter/parsers/tree-sitter-less/",
                    "945f52c94250309073a96bbfbc5bcd57ff2bde49/darwin-x64.dylib"
                ),
            ),
            (
                "linux",
                "aarch64",
                concat!(
                    "https://tree-sitter-parsers.s3.us-east-1.amazonaws.com/tree-sitter/parsers/tree-sitter-less/",
                    "945f52c94250309073a96bbfbc5bcd57ff2bde49/linux-arm64.so"
                ),
            ),
            (
                "linux",
                "x86_64",
                concat!(
                    "https://tree-sitter-parsers.s3.us-east-1.amazonaws.com/tree-sitter/parsers/tree-sitter-less/",
                    "945f52c94250309073a96bbfbc5bcd57ff2bde49/linux-x64.so"
                ),
            ),
            (
                "windows",
                "x86_64",
                concat!(
                    "https://tree-sitter-parsers.s3.us-east-1.amazonaws.com/tree-sitter/parsers/tree-sitter-less/",
                    "945f52c94250309073a96bbfbc5bcd57ff2bde49/win32-x64.dll"
                ),
            ),
        ],
    }]
}

fn current_platform() -> Result<(&'static str, &'static str), LoaderError> {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        return Err(LoaderError::UnsupportedPlatform {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        });
    };

    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        return Err(LoaderError::UnsupportedPlatform {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        });
    };

    Ok((os, arch))
}

fn get_cache_dir() -> Result<PathBuf, LoaderError> {
    if let Ok(dir) = std::env::var("CODEMOD_PARSER_CACHE_DIR") {
        return Ok(PathBuf::from(dir));
    }
    dirs::cache_dir()
        .map(|d| d.join("codemod").join("tree-sitter-parsers"))
        .ok_or(LoaderError::NoCacheDir)
}

fn ensure_parser_cached(
    def: &DynamicLanguageDefinition,
    cache_dir: &Path,
) -> Result<PathBuf, LoaderError> {
    let (os, arch) = current_platform()?;

    let url = def
        .urls
        .iter()
        .find(|(o, a, _)| *o == os && *a == arch)
        .map(|(_, _, u)| *u)
        .ok_or_else(|| LoaderError::UnsupportedPlatform {
            os: os.to_string(),
            arch: arch.to_string(),
        })?;

    let ext = if cfg!(target_os = "windows") {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    };

    let filename = format!("{}.{}", def.name, ext);
    let parser_dir = cache_dir.join(def.name);
    let cached_path = parser_dir.join(&filename);

    if cached_path.exists() {
        log::debug!("Parser {} already cached at {:?}", def.name, cached_path);
        return Ok(cached_path);
    }

    log::info!("Downloading tree-sitter parser for {} ...", def.name);
    std::fs::create_dir_all(&parser_dir)?;

    let response = reqwest::blocking::get(url)
        .map_err(|e| LoaderError::Download(format!("HTTP request failed: {e}")))?;

    if !response.status().is_success() {
        return Err(LoaderError::Download(format!(
            "HTTP {} for {}",
            response.status(),
            url
        )));
    }

    let bytes = response
        .bytes()
        .map_err(|e| LoaderError::Download(format!("Failed to read response body: {e}")))?;

    std::fs::write(&cached_path, &bytes)?;
    log::info!(
        "Downloaded {} parser to {:?} ({} bytes)",
        def.name,
        cached_path,
        bytes.len()
    );

    Ok(cached_path)
}

/// Register all dynamic language parsers, downloading any that are missing.
///
/// This should be called once before using dynamic languages.
/// Returns Ok(()) if all parsers were registered successfully.
pub fn register_all() -> Result<(), LoaderError> {
    let cache_dir = get_cache_dir()?;
    let definitions = get_definitions();

    let mut registrations = Vec::new();

    for def in definitions {
        let lib_path = ensure_parser_cached(def, &cache_dir)?;
        registrations.push(Registration {
            lang_name: def.name.to_string(),
            lib_path,
            symbol: def.symbol.to_string(),
            meta_var_char: None,
            expando_char: Some(def.expando_char),
            extensions: def.extensions.iter().map(|s| s.to_string()).collect(),
        });
    }

    unsafe {
        DynamicLang::register(registrations).map_err(|e| LoaderError::Register(format!("{e}")))?;
    }

    Ok(())
}

static INIT: Once = Once::new();
static INIT_ERROR: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Initialize dynamic language parsers (lazy, called at most once).
///
/// On first call, downloads and registers all dynamic parsers.
/// Subsequent calls are no-ops. If initialization failed, returns the error
/// on every call.
pub fn init() -> Result<(), LoaderError> {
    INIT.call_once(|| {
        if let Err(e) = register_all() {
            log::warn!("Failed to initialize dynamic parsers: {e}");
            let _ = INIT_ERROR.set(e.to_string());
        }
    });

    if let Some(msg) = INIT_ERROR.get() {
        Err(LoaderError::Register(msg.clone()))
    } else {
        Ok(())
    }
}
