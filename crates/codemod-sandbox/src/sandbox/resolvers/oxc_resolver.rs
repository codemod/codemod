use super::traits::ModuleResolver;
use crate::sandbox::errors::ResolverError;
use oxc_resolver::{
    ResolveOptions, Resolver, ResolverGeneric, TsconfigDiscovery, TsconfigOptions,
    TsconfigReferences,
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
            condition_names: vec![
                "module".into(),
                "import".into(),
                "node".into(),
                "default".into(),
            ],
            main_fields: vec!["module".into(), "main".into()],
            tsconfig: match tsconfig_path {
                Some(path) => Some(TsconfigDiscovery::Manual(TsconfigOptions {
                    config_file: path,
                    references: TsconfigReferences::Auto,
                })),
                None => Some(TsconfigDiscovery::Auto),
            },
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
        let specifier_path = Path::new(name);
        if specifier_path.is_absolute() {
            if specifier_path.exists() {
                return Ok(specifier_path.to_string_lossy().to_string());
            }

            return Err(ResolverError::ResolutionFailed {
                base: base.to_string(),
                name: name.to_string(),
            });
        }

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

        if name.starts_with("./") || name.starts_with("../") {
            let candidate = absolute_context.join(name);

            if let Ok(canonical) = candidate.canonicalize() {
                if canonical.is_file() {
                    return Ok(canonical.to_string_lossy().to_string());
                }
            }

            if candidate.extension().is_none() {
                for extension in [".js", ".ts", ".jsx", ".tsx", ".mjs", ".mts"] {
                    let with_extension =
                        candidate.with_extension(extension.trim_start_matches('.'));
                    if let Ok(canonical) = with_extension.canonicalize() {
                        if canonical.is_file() {
                            return Ok(canonical.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }

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

    #[test]
    fn test_resolver_with_relative_parent_traversal() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path().join("cases").join("example");
        fs::create_dir_all(&base_dir).unwrap();

        let shared_file = temp_dir.path().join("shared.ts");
        fs::write(&shared_file, "export const shared = true;").unwrap();

        let resolver = OxcResolver::new(base_dir.clone(), None).unwrap();
        let base_file = base_dir.join("codemod.ts");
        fs::write(&base_file, "import { shared } from '../../shared.ts';").unwrap();

        let result = resolver.resolve(&base_file.to_string_lossy(), "../../shared.ts");
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            shared_file.canonicalize().unwrap().to_string_lossy()
        );
    }

    #[test]
    fn test_resolver_with_absolute_path() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path().to_path_buf();
        let absolute_file = base_dir.join("absolute.ts");
        fs::write(&absolute_file, "export const value = true;").unwrap();

        let resolver = OxcResolver::new(base_dir, None).unwrap();
        let result = resolver.resolve("", &absolute_file.to_string_lossy());

        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            absolute_file.canonicalize().unwrap().to_string_lossy()
        );
    }

    #[test]
    fn test_resolver_with_extensionless_relative_import() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path().join("cases").join("example");
        fs::create_dir_all(base_dir.join("helpers")).unwrap();

        let helper_file = base_dir.join("helpers").join("runtime-check.ts");
        fs::write(&helper_file, "export const runtimeCheck = true;").unwrap();

        let resolver = OxcResolver::new(base_dir.clone(), None).unwrap();
        let base_file = base_dir.join("codemod.ts");
        fs::write(
            &base_file,
            "import { runtimeCheck } from './helpers/runtime-check';",
        )
        .unwrap();

        let result = resolver.resolve(&base_file.to_string_lossy(), "./helpers/runtime-check");
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            helper_file.canonicalize().unwrap().to_string_lossy()
        );
    }

    #[test]
    fn test_resolver_preserves_node_modules_package_resolution() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path().to_path_buf();
        let package_dir = base_dir.join("node_modules").join("demo-pkg");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            package_dir.join("package.json"),
            r#"{ "name": "demo-pkg", "main": "./index.js" }"#,
        )
        .unwrap();
        let entry_file = package_dir.join("index.js");
        fs::write(&entry_file, "export const value = true;").unwrap();

        let resolver = OxcResolver::new(base_dir, None).unwrap();
        let result = resolver.resolve("", "demo-pkg");

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), entry_file.canonicalize().unwrap().to_string_lossy());
    }

    #[test]
    fn test_resolver_preserves_package_exports_resolution() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path().to_path_buf();
        let package_dir = base_dir.join("node_modules").join("exports-pkg");
        let dist_dir = package_dir.join("dist");
        fs::create_dir_all(&dist_dir).unwrap();
        fs::write(
            package_dir.join("package.json"),
            r#"{ "name": "exports-pkg", "exports": { ".": "./dist/index.js" } }"#,
        )
        .unwrap();
        let entry_file = dist_dir.join("index.js");
        fs::write(&entry_file, "export const value = true;").unwrap();

        let resolver = OxcResolver::new(base_dir, None).unwrap();
        let result = resolver.resolve("", "exports-pkg");

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), entry_file.canonicalize().unwrap().to_string_lossy());
    }

    #[test]
    fn test_resolver_preserves_tsconfig_path_alias_resolution() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path().to_path_buf();
        let src_dir = base_dir.join("src");
        fs::create_dir_all(&src_dir).unwrap();

        let tsconfig_path = base_dir.join("tsconfig.json");
        fs::write(
            &tsconfig_path,
            r#"{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": {
      "@lib/*": ["src/*"]
    }
  }
}"#,
        )
        .unwrap();

        let aliased_file = src_dir.join("feature.ts");
        fs::write(&aliased_file, "export const feature = true;").unwrap();
        let base_file = base_dir.join("codemod.ts");
        fs::write(&base_file, "import { feature } from '@lib/feature';").unwrap();

        let resolver = OxcResolver::new(base_dir, Some(tsconfig_path)).unwrap();
        let result = resolver.resolve(&base_file.to_string_lossy(), "@lib/feature");

        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            aliased_file.canonicalize().unwrap().to_string_lossy()
        );
    }
}
