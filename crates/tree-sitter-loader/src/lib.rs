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
    #[error("Cached parser is locked: {0}")]
    LockedCache(#[source] std::io::Error),
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
        DynamicLanguageDefinition {
            name: "toml",
            symbol: "tree_sitter_toml",
            extensions: &["toml"],
            expando_char: '_',
            urls: parser_urls!(
                "tree-sitter-toml",
                "64b56832c2cffe41758f28e05c756a3a98d16f41"
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
    let matches_symbol = |name: &[u8]| name == symbol.as_bytes() || name == underscored.as_bytes();

    // Stripped PE DLLs generally expose parser entry points only through the
    // export table, while ELF and Mach-O artifacts may retain regular symbols.
    // Check both representations so valid Windows parsers are not repeatedly
    // treated as corrupt and replaced while another process has them loaded.
    if file
        .exports()
        .is_ok_and(|exports| exports.iter().any(|export| matches_symbol(export.name())))
    {
        return true;
    }

    file.symbols().any(|candidate| {
        candidate
            .name()
            .is_ok_and(|name| matches_symbol(name.as_bytes()))
    })
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
            if let Err(error) = std::fs::remove_file(&cached_path) {
                if cfg!(target_os = "windows") && error.raw_os_error() == Some(5) {
                    return Err(LoaderError::LockedCache(error));
                }
                return Err(LoaderError::Io(error));
            }
        } else {
            log::debug!("Parser {} already cached at {:?}", def.name, cached_path);
            return Ok(cached_path);
        }
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

fn prepare_registrations(
    definitions: &[DynamicLanguageDefinition],
    cache_dir: &Path,
) -> Result<Vec<Registration>, LoaderError> {
    let mut registrations = Vec::new();
    let mut failures = Vec::new();

    for def in definitions {
        match ensure_parser_cached(def, cache_dir) {
            Ok(lib_path) => registrations.push(Registration {
                lang_name: def.name.to_string(),
                lib_path,
                symbol: def.symbol.to_string(),
                meta_var_char: None,
                expando_char: Some(def.expando_char),
                extensions: def.extensions.iter().map(|s| s.to_string()).collect(),
            }),
            Err(error @ LoaderError::LockedCache(_)) => {
                log::warn!(
                    "Dynamic parser {} is unavailable and will be skipped: {error}",
                    def.name
                );
                failures.push(format!("{}: {error}", def.name));
            }
            Err(error) => return Err(error),
        }
    }

    if registrations.is_empty() {
        return Err(LoaderError::Register(format!(
            "No dynamic parsers are available: {}",
            failures.join("; ")
        )));
    }

    Ok(registrations)
}

/// Register available dynamic language parsers, downloading any that are missing.
///
/// This should be called once before using dynamic languages. A failure to prepare
/// one parser does not prevent unrelated parsers from being registered.
pub fn register_all() -> Result<(), LoaderError> {
    let cache_dir = get_cache_dir()?;
    let registrations = prepare_registrations(get_definitions(), &cache_dir)?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_propagates_non_locking_failures() {
        let cache = tempfile::tempdir().expect("create parser cache");
        let definition = DynamicLanguageDefinition {
            name: "unavailable",
            symbol: "tree_sitter_unavailable",
            extensions: &["unavailable"],
            expando_char: '_',
            urls: &[],
        };

        let error = match prepare_registrations(&[definition], cache.path()) {
            Ok(_) => panic!("non-locking parser failures must propagate"),
            Err(error) => error,
        };

        assert!(matches!(error, LoaderError::UnsupportedPlatform { .. }));
    }

    #[test]
    #[ignore = "downloads published parser artifacts"]
    fn published_parsers_export_expected_symbols() {
        let cache = tempfile::tempdir().expect("create parser cache");
        let registrations = prepare_registrations(get_definitions(), cache.path())
            .expect("download and validate published parsers");

        let registered_names = registrations
            .iter()
            .map(|registration| registration.lang_name.as_str())
            .collect::<Vec<_>>();
        let expected_names = get_definitions()
            .iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();

        assert_eq!(registered_names, expected_names);
    }

    #[cfg(target_os = "windows")]
    mod windows {
        use super::*;

        const LESS_URL: &str = parser_url!(
            "tree-sitter-less",
            "945f52c94250309073a96bbfbc5bcd57ff2bde49",
            "win32-x64.dll"
        );
        const XML_URL: &str = parser_url!(
            "tree-sitter-xml",
            "4b64dd3a03ec002258d6268d712fd93716d6ab57",
            "win32-x64.dll"
        );

        static BAD_URLS: &[(&str, &str, &str)] = &[("windows", "x86_64", LESS_URL)];
        static GOOD_URLS: &[(&str, &str, &str)] = &[("windows", "x86_64", XML_URL)];

        fn download_fixture(url: &str) -> Vec<u8> {
            reqwest::blocking::get(url)
                .expect("download Windows parser fixture")
                .error_for_status()
                .expect("download successful Windows parser fixture")
                .bytes()
                .expect("read Windows parser fixture")
                .to_vec()
        }

        #[test]
        #[ignore = "downloads published parser artifacts"]
        fn locked_invalid_cache_entry_does_not_disable_unrelated_parser() {
            let cache = tempfile::tempdir().expect("create parser cache");
            let less_dir = cache.path().join("less");
            let xml_dir = cache.path().join("xml");
            std::fs::create_dir_all(&less_dir).expect("create less cache directory");
            std::fs::create_dir_all(&xml_dir).expect("create xml cache directory");

            let less_bytes = download_fixture(LESS_URL);
            let xml_bytes = download_fixture(XML_URL);
            let less_path = less_dir.join("less.dll");
            let xml_path = xml_dir.join("xml.dll");
            std::fs::write(&less_path, &less_bytes).expect("cache less parser fixture");
            std::fs::write(&xml_path, &xml_bytes).expect("cache XML parser fixture");

            assert!(
                cached_parser_has_symbol(&less_path, "tree_sitter_less"),
                "the parser symbol is stored in the PE export table"
            );
            assert!(cached_parser_has_symbol(&xml_path, "tree_sitter_xml"));

            // DynamicLang retains the library handle, reproducing the Windows image
            // lock held by another long-lived codemod process.
            unsafe {
                DynamicLang::register(vec![Registration {
                    lang_name: "loaded-less".to_string(),
                    lib_path: less_path,
                    symbol: "tree_sitter_less".to_string(),
                    meta_var_char: None,
                    expando_char: Some('_'),
                    extensions: vec!["loaded-less".to_string()],
                }])
                .expect("load and pin less parser fixture");
            }

            let definitions = [
                DynamicLanguageDefinition {
                    name: "less",
                    symbol: "tree_sitter_missing",
                    extensions: &["less"],
                    expando_char: '_',
                    urls: BAD_URLS,
                },
                DynamicLanguageDefinition {
                    name: "xml",
                    symbol: "tree_sitter_xml",
                    extensions: &["xml"],
                    expando_char: '_',
                    urls: GOOD_URLS,
                },
            ];

            let registrations = prepare_registrations(&definitions, cache.path())
                .expect("prepare the unrelated parser despite the locked cache entry");

            assert_eq!(registrations.len(), 1);
            assert_eq!(registrations[0].lang_name, "xml");

            // Windows cannot remove a directory containing a loaded DLL. Leave this
            // process-scoped temporary cache for the runner to clean up after exit.
            std::mem::forget(cache);
        }
    }
}
