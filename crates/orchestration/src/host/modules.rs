//! Virtual modules: transform artifacts the bundle registers by name
//! (`registerModule(specifier, source)`) and then imports with a dynamic
//! `import(specifier)`, so each artifact is evaluated once per run by the
//! engine's own module cache. The source is declared verbatim, exactly as the
//! sandbox's in-memory loader declares it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rquickjs::loader::{Loader, Resolver};
use rquickjs::{module::Declared, Ctx, Error, Module, Result};

#[derive(Clone, Default)]
pub struct ModuleRegistry {
    sources: Arc<Mutex<HashMap<String, String>>>,
}

impl ModuleRegistry {
    pub fn register(&self, specifier: String, source: String) {
        if let Ok(mut sources) = self.sources.lock() {
            sources.entry(specifier).or_insert(source);
        }
    }

    fn contains(&self, specifier: &str) -> bool {
        self.sources
            .lock()
            .map(|sources| sources.contains_key(specifier))
            .unwrap_or(false)
    }

    pub fn resolver(&self) -> RegistryResolver {
        RegistryResolver(self.clone())
    }

    pub fn loader(&self) -> RegistryLoader {
        RegistryLoader(self.clone())
    }
}

pub struct RegistryResolver(ModuleRegistry);

impl Resolver for RegistryResolver {
    fn resolve(&mut self, _ctx: &Ctx<'_>, base: &str, name: &str) -> Result<String> {
        if self.0.contains(name) {
            Ok(name.to_string())
        } else {
            Err(Error::new_resolving(base, name))
        }
    }
}

pub struct RegistryLoader(ModuleRegistry);

impl Loader for RegistryLoader {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str) -> Result<Module<'js, Declared>> {
        let source = self
            .0
            .sources
            .lock()
            .ok()
            .and_then(|sources| sources.get(name).cloned());
        match source {
            Some(source) => Module::declare(ctx.clone(), name, source),
            None => Err(Error::new_loading(name)),
        }
    }
}
