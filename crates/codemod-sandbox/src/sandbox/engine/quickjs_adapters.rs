use crate::codemod_modules::{codemod_module, codemod_module_specifier};
use crate::sandbox::resolvers::ModuleResolver;
use crate::utils::transpiler;
use rquickjs::{
    loader::{Loader, Resolver},
    module, Ctx, Error, Module, Result,
};
use std::sync::Arc;

/// QuickJS-compatible resolver adapter that uses any ModuleResolver
pub struct QuickJSResolver<R: ModuleResolver> {
    resolver: Arc<R>,
}

impl<R: ModuleResolver> QuickJSResolver<R> {
    pub fn new(resolver: Arc<R>) -> Self {
        Self { resolver }
    }
}

impl<R: ModuleResolver> Resolver for QuickJSResolver<R> {
    fn resolve(&mut self, _ctx: &Ctx<'_>, base: &str, name: &str) -> Result<String> {
        // Use the configured ModuleResolver to resolve the module
        match self.resolver.resolve(base, name) {
            Ok(resolved_path) => Ok(resolved_path),
            Err(_) => Err(Error::new_resolving(base, name)),
        }
    }
}

/// Resolves the codemod runtime modules a runtime offers. `codemod/ast-grep`
/// and the older `codemod:ast-grep` resolve to the same module, which loaders
/// register under `codemod/ast-grep`.
pub struct CodemodModuleResolver {
    modules: Vec<&'static str>,
}

impl CodemodModuleResolver {
    /// Offer these modules (names from [`crate::codemod_modules::CODEMOD_MODULES`]).
    pub fn new(modules: &[&'static str]) -> Self {
        Self {
            modules: modules.to_vec(),
        }
    }

    /// The modules every transform gets, and `llm` when `llm` is set (it
    /// makes network requests, so it needs the fetch capability).
    pub fn offering(llm: bool) -> Self {
        let mut modules = vec!["ast-grep", "workflow", "metrics", "runtime"];
        if llm {
            modules.push("llm");
        }
        Self { modules }
    }
}

impl Resolver for CodemodModuleResolver {
    fn resolve(&mut self, _ctx: &Ctx<'_>, base: &str, name: &str) -> Result<String> {
        match codemod_module(name).filter(|module| self.modules.contains(module)) {
            Some(module) => Ok(codemod_module_specifier(module)),
            None => Err(Error::new_resolving(base, name)),
        }
    }
}

/// QuickJS-compatible loader adapter
pub struct QuickJSLoader;

impl Loader for QuickJSLoader {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str) -> Result<Module<'js, module::Declared>> {
        let path = std::path::Path::new(name);
        if path.exists() && path.is_file() {
            let source = std::fs::read_to_string(path).map_err(|_| Error::new_loading(name))?;

            // Check if the file is a TypeScript file that needs transpilation
            let extension = path.extension().and_then(|ext| ext.to_str());
            let needs_transpilation = matches!(extension, Some("ts") | Some("mts") | Some("cts"));
            let file_name = path
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("anon.ts"))
                .to_string_lossy();

            if needs_transpilation {
                let transpiled_bytes = transpiler::transpile(source, file_name.to_string())
                    .map_err(|err| {
                        Error::new_loading_message(name, format!("Transpilation failed: {err}"))
                    })?;
                Module::declare(ctx.clone(), name, transpiled_bytes.as_slice())
            } else {
                Module::declare(ctx.clone(), name, source.as_bytes())
            }
        } else {
            Err(Error::new_loading(name))
        }
    }
}
