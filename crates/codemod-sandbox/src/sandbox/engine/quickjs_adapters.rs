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
///
/// Every `codemod/...` and `codemod:...` specifier is answered here: one this
/// runtime does not offer is an error, never looked up in `node_modules`,
/// where the `codemod` package's files for Node.js would be found.
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
        let Some(rest) = name
            .strip_prefix("codemod/")
            .or_else(|| name.strip_prefix("codemod:"))
        else {
            return Err(Error::new_resolving(base, name));
        };
        let unavailable = |why: String| Err(Error::new_loading_message(name, why));
        match codemod_module(name) {
            Some(module) if self.modules.contains(&module) => Ok(codemod_module_specifier(module)),
            Some("llm") => unavailable(
                "it needs the fetch capability: add `fetch` to `capabilities` in codemod.yaml, or pass --allow-fetch".to_string(),
            ),
            Some(_) => unavailable("it is not available in this part of a workflow".to_string()),
            None if rest.starts_with("ast-grep/langs/") || rest.starts_with("langs/") => unavailable(
                "it has types only: import it with `import type`".to_string(),
            ),
            None => unavailable(
                "it is part of the codemod package for Node.js, not of the runtime the codemod CLI runs code in".to_string(),
            ),
        }
    }
}

/// Add the codemod modules of transforms and workflow callbacks to an LLRT
/// module loader. Gives the resolver that names them and the loader, so the
/// two cannot disagree about `codemod/llm`.
#[cfg(feature = "native")]
#[macro_export]
macro_rules! with_codemod_modules {
    ($loader:expr, llm: $llm:expr) => {{
        let llm: bool = $llm;
        let mut loader = $loader
            .with_module("codemod/ast-grep", $crate::AstGrepModule)
            .with_module(
                "codemod/workflow",
                $crate::workflow_global::WorkflowGlobalModule,
            )
            .with_module("codemod/metrics", $crate::metrics::MetricsModule)
            .with_module(
                "codemod/runtime",
                $crate::sandbox::runtime_module::RuntimeModule,
            );
        if llm {
            loader = loader.with_module("codemod/llm", $crate::llm::LlmModule);
        }
        (
            $crate::sandbox::engine::quickjs_adapters::CodemodModuleResolver::offering(llm),
            loader,
        )
    }};
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

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(resolver: &mut CodemodModuleResolver, name: &str) -> Result<String> {
        let runtime = rquickjs::Runtime::new().unwrap();
        let context = rquickjs::Context::full(&runtime).unwrap();
        context.with(|ctx| resolver.resolve(&ctx, "/package/scripts/codemod.ts", name))
    }

    #[test]
    fn both_spellings_resolve_to_one_module() {
        let mut resolver = CodemodModuleResolver::offering(false);
        assert_eq!(
            resolve(&mut resolver, "codemod/ast-grep").unwrap(),
            "codemod/ast-grep"
        );
        assert_eq!(
            resolve(&mut resolver, "codemod:ast-grep").unwrap(),
            "codemod/ast-grep"
        );
    }

    #[test]
    fn other_resolvers_are_asked_only_about_names_outside_the_namespace() {
        let mut resolver = CodemodModuleResolver::offering(false);
        // A resolving error lets the next resolver try: a package, a file.
        for name in ["lodash", "./codemod/local.ts", "codemodx/thing"] {
            assert!(
                matches!(resolve(&mut resolver, name), Err(Error::Resolving { .. })),
                "{name}"
            );
        }
        // Anything else would find the codemod package's Node.js files in
        // node_modules.
        for (name, reason) in [
            ("codemod/llm", "fetch capability"),
            ("codemod:llm", "fetch capability"),
            ("codemod/testing", "for Node.js"),
            ("codemod/main", "for Node.js"),
            ("codemod/ast-grep/langs/tsx", "import type"),
        ] {
            match resolve(&mut resolver, name) {
                Err(Error::Loading { message, .. }) => {
                    assert!(message.unwrap_or_default().contains(reason), "{name}")
                }
                other => panic!("{name}: {other:?}"),
            }
        }
        assert!(resolve(&mut CodemodModuleResolver::offering(true), "codemod/llm").is_ok());
    }
}
