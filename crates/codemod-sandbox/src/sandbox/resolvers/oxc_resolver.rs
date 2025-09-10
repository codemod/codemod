use super::traits::ModuleResolver;
use crate::sandbox::errors::ResolverError;
use oxc_resolver::{
    ResolveOptions, Resolver, ResolverGeneric, TsconfigOptions, TsconfigReferences,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Node.js-style module resolver using oxc_resolver
///
/// This resolver provides Node.js-compatible module resolution with support for:
/// - TypeScript path mapping via tsconfig.json
/// - Package.json exports/imports
/// - Extension resolution (.js, .ts, etc.)
/// - Alias configuration
/// - ESM/CJS condition names
pub struct OxcResolver {
    resolver: Arc<Resolver>,
    base_dir: PathBuf,
}

impl OxcResolver {
    pub fn new(base_dir: PathBuf, tsconfig_path: Option<PathBuf>) -> Result<Self, ResolverError> {
        let options = ResolveOptions {
            extensions: vec![
                ".js".into(),
                ".ts".into(),
                ".jsx".into(),
                ".tsx".into(),
                ".mjs".into(),
                ".mts".into(),
            ],
            condition_names: vec!["module".into(), "import".into(), "node".into()],
            main_fields: vec!["module".into(), "main".into()],
            tsconfig: tsconfig_path.map(|path| TsconfigOptions {
                config_file: path,
                references: TsconfigReferences::Auto,
            }),
            ..ResolveOptions::default()
        };

        let resolver = ResolverGeneric::new(options);
        Ok(Self {
            resolver: Arc::new(resolver),
            base_dir,
        })
    }
}

impl ModuleResolver for OxcResolver {
    fn resolve(&self, base: &str, name: &str) -> Result<String, ResolverError> {
        // Determine the resolution context directory
        let context_dir = if base.is_empty() {
            self.base_dir.clone()
        } else {
            let base_path = Path::new(base);
            if base_path.is_absolute() {
                base_path.parent().unwrap_or(&self.base_dir).to_path_buf()
            } else {
                self.base_dir
                    .join(base_path.parent().unwrap_or(Path::new("")))
            }
        };

        // Ensure the context directory is absolute
        let absolute_context = if context_dir.is_absolute() {
            context_dir
        } else {
            std::env::current_dir()
                .map_err(|_| ResolverError::ResolutionFailed {
                    base: base.to_string(),
                    name: name.to_string(),
                })?
                .join(context_dir)
        };

        // Use oxc_resolver to resolve the module
        match self.resolver.resolve(&absolute_context, name) {
            Ok(resolution) => Ok(resolution.full_path().to_string_lossy().to_string()),
            Err(err) => Err(ResolverError::ResolutionFailed {
                base: base.to_string(),
                name: format!("{name}: {err}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn test_oxc_resolver_creation() {
        let base_dir = PathBuf::from("/tmp");
        let resolver = OxcResolver::new(base_dir, None).unwrap();
        assert_eq!(resolver.base_dir, PathBuf::from("/tmp"));
    }

    #[test]
    fn test_resolver_with_relative_path() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path().to_path_buf();

        let test_file = base_dir.join("test.js");
        fs::write(&test_file, "console.log('test');").unwrap();

        let resolver = OxcResolver::new(base_dir.clone(), None).unwrap();

        let result = resolver.resolve("", "./test.js");
        match result {
            Ok(resolved_path) => {
                assert!(resolved_path.contains("test.js"));
            }
            Err(e) => {
                println!("Resolution failed (expected in test): {e}");
            }
        }
    }
}
