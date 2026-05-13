use ast_grep_dynamic::{DynamicLang, Registration};
use object::{Object, ObjectSymbol};
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

macro_rules! parser_url {
    ($parser:literal, $revision:literal, $artifact:literal) => {
        concat!(
            "https://tree-sitter-parsers.s3.us-east-1.amazonaws.com/tree-sitter/parsers/",
            $parser,
            "/",
            $revision,
            "/",
            $artifact
        )
    };
}

macro_rules! parser_urls {
    ($parser:literal, $revision:literal) => {
        &[
            (
                "macos",
                "aarch64",
                parser_url!($parser, $revision, "darwin-arm64.dylib"),
            ),
            (
                "macos",
                "x86_64",
                parser_url!($parser, $revision, "darwin-x64.dylib"),
            ),
            (
                "linux",
                "aarch64",
                parser_url!($parser, $revision, "linux-arm64.so"),
            ),
            (
                "linux",
                "x86_64",
                parser_url!($parser, $revision, "linux-x64.so"),
            ),
            (
                "windows",
                "x86_64",
                parser_url!($parser, $revision, "win32-x64.dll"),
            ),
        ]
    };
}

fn get_definitions() -> &'static [DynamicLanguageDefinition] {
    &[
        DynamicLanguageDefinition {
            name: "less",
            symbol: "tree_sitter_less",
            extensions: &["less"],
            expando_char: '_',
            urls: parser_urls!(
                "tree-sitter-less",
                "945f52c94250309073a96bbfbc5bcd57ff2bde49"
            ),
        },
        DynamicLanguageDefinition {
            name: "xml",
            symbol: "tree_sitter_xml",
            extensions: &[
                "xml", "csproj", "props", "targets", "config", "resx", "xaml",
            ],
            expando_char: '_',
            urls: parser_urls!(
                "tree-sitter-xml",
                "4b64dd3a03ec002258d6268d712fd93716d6ab57"
            ),
        },
    ]
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

fn cached_parser_has_symbol(path: &Path, symbol: &str) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(file) = object::File::parse(bytes.as_slice()) else {
        return false;
    };

    let underscored = format!("_{symbol}");

    file.symbols().any(|candidate| {
        candidate
            .name()
            .is_ok_and(|name| name == symbol || name == underscored)
    })
}

fn download_parser(url: &'static str) -> Result<Vec<u8>, LoaderError> {
    std::thread::Builder::new()
        .name("tree-sitter-parser-download".to_string())
        .spawn(move || {
            let response = reqwest::blocking::get(url)
                .map_err(|e| LoaderError::Download(format!("HTTP request failed: {e}")))?;

            if !response.status().is_success() {
                return Err(LoaderError::Download(format!(
                    "HTTP {} for {}",
                    response.status(),
                    url
                )));
            }

            response
                .bytes()
                .map(|bytes| bytes.to_vec())
                .map_err(|e| LoaderError::Download(format!("Failed to read response body: {e}")))
        })
        .map_err(|e| LoaderError::Download(format!("Failed to spawn download thread: {e}")))?
        .join()
        .map_err(|_| LoaderError::Download("download thread panicked".to_string()))?
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
        if !cached_parser_has_symbol(&cached_path, def.symbol) {
            log::warn!(
                "Cached parser {} at {:?} does not export {}; redownloading",
                def.name,
                cached_path,
                def.symbol
            );
            std::fs::remove_file(&cached_path)?;
        } else {
            log::debug!("Parser {} already cached at {:?}", def.name, cached_path);
            return Ok(cached_path);
        }
    }

    log::info!("Downloading tree-sitter parser for {} ...", def.name);
    std::fs::create_dir_all(&parser_dir)?;

    let bytes = download_parser(url)?;
    std::fs::write(&cached_path, &bytes)?;

    if !cached_parser_has_symbol(&cached_path, def.symbol) {
        let _ = std::fs::remove_file(&cached_path);
        return Err(LoaderError::Download(format!(
            "Downloaded parser for {} does not export {}",
            def.name, def.symbol
        )));
    }

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
