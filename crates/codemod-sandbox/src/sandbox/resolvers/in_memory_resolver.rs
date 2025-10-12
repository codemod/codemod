/// in-memory module resolver for memory-only execution
use crate::sandbox::errors::ResolverError;
use crate::sandbox::resolvers::ModuleResolver;
use rquickjs::{loader::Loader, module, Ctx, Error, Module};
use std::collections::HashMap;
use std::sync::Arc;

/// A resolver that can optionally resolve modules from an in-memory map
#[derive(Debug, Clone)]
pub struct InMemoryResolver {
    /// Optional in-memory module mappings
    /// Key: module name or path, Value: resolved path or virtual identifier
    modules: Arc<HashMap<String, String>>,
    /// In-memory module source code
    /// Key: resolved path, Value: source code content
    sources: Arc<HashMap<String, String>>,
}

impl InMemoryResolver {
    /// Create a new in-memory resolver with no module mappings
    pub fn new() -> Self {
        Self {
            modules: Arc::new(HashMap::new()),
            sources: Arc::new(HashMap::new()),
        }
    }

    /// Create a new resolver with predefined module mappings
    ///
    /// # Example
    /// ```
    /// use std::collections::HashMap;
    /// use codemod_sandbox::sandbox::resolvers::in_memory_resolver::InMemoryResolver;
    ///
    /// let mut modules = HashMap::new();
    /// modules.insert("utils".to_string(), "/__virtual/utils.js".to_string());
    /// let resolver = InMemoryResolver::with_modules(modules);
    /// ```
    pub fn with_modules(modules: HashMap<String, String>) -> Self {
        Self {
            modules: Arc::new(modules),
            sources: Arc::new(HashMap::new()),
        }
    }

    /// Add a module mapping after construction
    pub fn add_module(&mut self, name: String, resolved_path: String) {
        Arc::make_mut(&mut self.modules).insert(name, resolved_path);
    }

    /// Add a module with both its resolved path and source code
    pub fn add_module_with_source(&mut self, name: String, resolved_path: String, source: String) {
        Arc::make_mut(&mut self.modules).insert(name.clone(), resolved_path.clone());
        Arc::make_mut(&mut self.sources).insert(resolved_path, source);
    }

    /// Set the source code for a resolved path
    pub fn set_source(&mut self, resolved_path: String, source: String) {
        Arc::make_mut(&mut self.sources).insert(resolved_path, source);
    }

    /// Get the source code for a resolved path
    pub fn get_source(&self, resolved_path: &str) -> Option<&String> {
        self.sources.get(resolved_path)
    }
}

impl Default for InMemoryResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleResolver for InMemoryResolver {
    fn resolve(&self, base: &str, name: &str) -> Result<String, ResolverError> {
        // First check if we have an exact mapping
        if let Some(resolved) = self.modules.get(name) {
            return Ok(resolved.clone());
        }

        // For relative imports, try to resolve them relative to base
        if name.starts_with("./") || name.starts_with("../") {
            // Simple path resolution without filesystem
            let base_path = std::path::Path::new(base);
            let parent = base_path
                .parent()
                .ok_or_else(|| ResolverError::ResolutionFailed {
                    base: base.to_string(),
                    name: name.to_string(),
                })?;

            let resolved_path = parent.join(name);
            let resolved_str =
                resolved_path
                    .to_str()
                    .ok_or_else(|| ResolverError::InvalidPath {
                        path: name.to_string(),
                    })?;

            // Check if this resolved path is in our modules map
            if let Some(mapped) = self.modules.get(resolved_str) {
                return Ok(mapped.clone());
            }

            return Ok(resolved_str.to_string());
        }

        // If no mapping found and it's not a relative import, fail
        Err(ResolverError::ResolutionFailed {
            base: base.to_string(),
            name: name.to_string(),
        })
    }
}

/// QuickJS-compatible loader that loads modules from in-memory sources
pub struct InMemoryLoader {
    resolver: Arc<InMemoryResolver>,
}

impl InMemoryLoader {
    pub fn new(resolver: Arc<InMemoryResolver>) -> Self {
        Self { resolver }
    }
}

impl Loader for InMemoryLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
    ) -> rquickjs::Result<Module<'js, module::Declared>> {
        let source = self.resolver.get_source(name).or_else(|| {
            self.resolver
                .get_source(name.strip_prefix("./").unwrap_or(name))
        });

        if let Some(source) = source {
            Module::declare(ctx.clone(), name, source.as_bytes())
        } else {
            Err(Error::new_loading(&format!(
                "Module '{}' not found in memory",
                name
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_in_memory_resolver_no_modules() {
        let resolver = InMemoryResolver::new();
        let result = resolver.resolve("/base/path.js", "some-module");
        assert!(result.is_err());
    }

    #[test]
    fn test_in_memory_resolver_with_module_mapping() {
        let mut modules = HashMap::new();
        modules.insert("lodash".to_string(), "/__virtual/lodash.js".to_string());

        let resolver = InMemoryResolver::with_modules(modules);
        let result = resolver.resolve("/base/path.js", "lodash");

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "/__virtual/lodash.js");
    }

    #[test]
    fn test_in_memory_resolver_add_module() {
        let mut resolver = InMemoryResolver::new();
        resolver.add_module("mymodule".to_string(), "/__virtual/mymodule.js".to_string());

        let result = resolver.resolve("/base/path.js", "mymodule");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "/__virtual/mymodule.js");
    }

    #[test]
    fn test_in_memory_resolver_with_source() {
        let mut resolver = InMemoryResolver::new();
        let source_code = "export const foo = 'bar';";

        resolver.add_module_with_source(
            "mymodule".to_string(),
            "/__virtual/mymodule.js".to_string(),
            source_code.to_string(),
        );

        let result = resolver.resolve("/base/path.js", "mymodule");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "/__virtual/mymodule.js");

        let retrieved_source = resolver.get_source("/__virtual/mymodule.js");
        assert!(retrieved_source.is_some());
        assert_eq!(retrieved_source.unwrap(), source_code);
    }
}
