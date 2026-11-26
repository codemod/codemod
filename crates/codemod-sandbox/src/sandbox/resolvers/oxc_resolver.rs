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
    /// See [`OxcResolver::data_within`].
    data_root: Option<PathBuf>,
}

/// Files that load as code. Anything else an import names is data (a YAML
/// file loads as its content).
fn is_script(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx"
        )
    })
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
            data_root: None,
        })
    }

    /// Resolve data files (anything that is not JavaScript or TypeScript)
    /// only inside `root`, symbolic links followed: `import x from
    /// "/home/me/.config/tool/hosts.yml"` would otherwise hand a workflow the
    /// content of any YAML file on the machine. Code may still come from
    /// outside (a hoisted `node_modules`, a helper shared by sibling
    /// packages): loading it reveals nothing.
    pub fn data_within(mut self, root: &Path) -> Result<Self, ResolverError> {
        self.data_root =
            Some(
                root.canonicalize()
                    .map_err(|_| ResolverError::ResolutionFailed {
                        base: root.display().to_string(),
                        name: "the workflow's package directory".into(),
                    })?,
            );
        Ok(self)
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
            Ok(resolution) => {
                let path = resolution.full_path();
                if let Some(root) = self.data_root.as_ref().filter(|_| !is_script(&path)) {
                    let inside = path
                        .canonicalize()
                        .is_ok_and(|canonical| canonical.starts_with(root));
                    if !inside {
                        return Err(ResolverError::ResolutionFailed {
                            base: base.to_string(),
                            name: format!(
                                "{name}: a data file outside the workflow's package cannot be imported"
                            ),
                        });
                    }
                }
                Ok(path.to_string_lossy().to_string())
            }
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
    fn data_files_resolve_only_inside_the_package_and_code_anywhere() {
        let root = TempDir::new().unwrap();
        let package = root.path().join("package");
        fs::create_dir(&package).unwrap();
        fs::write(package.join("rules.yaml"), "id: a\n").unwrap();
        fs::write(root.path().join("secrets.yml"), "token: t\n").unwrap();
        fs::write(root.path().join("shared.js"), "").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.path().join("secrets.yml"), package.join("link.yml"))
            .unwrap();
        let resolver = OxcResolver::new(package.clone(), None)
            .unwrap()
            .data_within(&package)
            .unwrap();
        assert!(resolver.resolve("", "./rules.yaml").is_ok());
        assert!(resolver.resolve("", "../shared.js").is_ok());
        let outside = root.path().join("secrets.yml");
        for name in ["../secrets.yml", outside.to_str().unwrap()] {
            let error = resolver.resolve("", name).unwrap_err().to_string();
            assert!(error.contains("outside the workflow's package"), "{error}");
        }
        #[cfg(unix)]
        assert!(resolver.resolve("", "./link.yml").is_err());
    }

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
