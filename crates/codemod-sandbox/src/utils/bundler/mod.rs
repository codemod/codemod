use crate::sandbox::resolvers::{oxc_resolver::OxcResolver, traits::ModuleResolver};
use crate::utils::bundler::module_analyzer::{ModuleAnalyzer, ModuleExports};
use crate::utils::bundler::module_transformer::ModuleTransformer;
use crate::utils::transpiler;
use std::sync::Arc;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};
use swc_core::common::SourceMap;
use swc_core::ecma::codegen::{text_writer::JsWriter, Emitter};
use swc_core::{
    common::{BytePos, FileName},
    ecma::{
        ast::{EsVersion, ModuleItem},
        parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax},
        visit::{VisitMutWith, VisitWith},
    },
};

mod module_analyzer;
mod module_transformer;

#[cfg(test)]
mod module_transformer_tests;

/// Represents a module in the dependency graph
#[derive(Debug, Clone)]
pub struct BundledModule {
    /// Unique module ID
    pub id: String,
    /// Resolved absolute path
    pub path: String,
    /// Transpiled JavaScript code
    pub code: String,
    /// Dependencies (module names as they appear in imports)
    pub dependencies: Vec<String>,
    /// Resolved dependency paths
    pub resolved_dependencies: Vec<String>,
    /// Export information
    pub exports: ModuleExports,
}

/// Configuration for the bundler
#[derive(Debug, Clone)]
pub struct BundlerConfig {
    /// Base directory for resolution
    pub base_dir: PathBuf,
    /// TypeScript configuration file path (optional)
    pub tsconfig_path: Option<PathBuf>,
    /// Runtime module system to use
    pub runtime_system: RuntimeSystem,
    /// Whether to include source maps
    pub source_maps: bool,
}

/// Runtime module system options
#[derive(Debug, Clone)]
pub enum RuntimeSystem {
    /// CommonJS-style module system
    CommonJS,
    /// Custom runtime with require/module.exports
    Custom,
}

/// Main bundler implementation
pub struct Bundler {
    resolver: Box<dyn ModuleResolver>,
    config: BundlerConfig,
    /// Maps resolved absolute paths to obfuscated numeric IDs
    module_id_map: HashMap<String, u32>,
    /// Counter for generating unique module IDs
    next_module_id: u32,
}

impl Bundler {
    /// Check if a module path is a built-in module (codemod:* prefix)
    pub fn is_builtin_module(module_path: &str) -> bool {
        module_path.starts_with("codemod:")
            || module_path.starts_with("node:")
            || module_path == "fs"
            || module_path == "path"
            || module_path == "os"
            || module_path == "child_process"
            || module_path == "console"
            || module_path == "process"
    }

    /// Create a new bundler instance
    pub fn new(config: BundlerConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let tsconfig_path = config.tsconfig_path.clone();

        let resolver: Box<dyn ModuleResolver> =
            Box::new(OxcResolver::new(config.base_dir.clone(), tsconfig_path)?);

        Ok(Self {
            resolver,
            config,
            module_id_map: HashMap::new(),
            next_module_id: 1, // Start from 1, reserve 0 for potential special use
        })
    }

    /// Bundle a JavaScript/TypeScript file and its dependencies
    pub fn bundle(&mut self, entry_path: &str) -> Result<BundleResult, Box<dyn std::error::Error>> {
        let mut visited = HashSet::new();
        let mut modules = HashMap::new();
        let mut dependency_graph = HashMap::new();
        let mut builtin_modules = HashSet::new();

        // Build dependency graph starting from entry point
        self.build_dependency_graph(
            entry_path,
            "",
            &mut visited,
            &mut modules,
            &mut dependency_graph,
            &mut builtin_modules,
        )?;

        // Generate bundled code
        let bundle_code =
            self.generate_bundle(&modules, &dependency_graph, entry_path, &builtin_modules)?;

        Ok(BundleResult {
            code: bundle_code,
            modules: modules.into_values().collect(),
            entry_module: entry_path.to_string(),
        })
    }

    /// Recursively build the dependency graph
    fn build_dependency_graph(
        &mut self,
        module_path: &str,
        base_path: &str,
        visited: &mut HashSet<String>,
        modules: &mut HashMap<String, BundledModule>,
        dependency_graph: &mut HashMap<String, Vec<String>>,
        builtin_modules: &mut HashSet<String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Resolve the module path
        let resolved_path = self.resolver.resolve(base_path, module_path)?;

        if visited.contains(&resolved_path) {
            return Ok(());
        }

        visited.insert(resolved_path.clone());

        // Read and parse the module
        let source_code = std::fs::read_to_string(&resolved_path)?;

        // Transpile the code
        let transpiled_code = self.transpile_module(&source_code, &resolved_path)?;

        let (dependencies, exports) = self.analyze_module(&transpiled_code, &resolved_path)?;

        // Resolve dependencies
        let mut resolved_dependencies = Vec::new();
        for dep in &dependencies {
            if Self::is_builtin_module(dep) {
                // Built-in modules are not bundled, but we track them
                resolved_dependencies.push(dep.to_string());
                builtin_modules.insert(dep.to_string());
            } else {
                let resolved_dep = self.resolver.resolve(&resolved_path, dep)?;
                resolved_dependencies.push(resolved_dep.clone());

                // Recursively process dependencies (skip built-ins)
                self.build_dependency_graph(
                    dep,
                    &resolved_path,
                    visited,
                    modules,
                    dependency_graph,
                    builtin_modules,
                )?;
            }
        }

        // Create bundled module
        let module = BundledModule {
            id: self.generate_module_id(&resolved_path),
            path: resolved_path.clone(),
            code: transpiled_code,
            dependencies: dependencies.clone(),
            resolved_dependencies: resolved_dependencies.clone(),
            exports,
        };

        modules.insert(resolved_path.clone(), module);
        dependency_graph.insert(resolved_path, resolved_dependencies);

        Ok(())
    }

    /// Analyze a module's imports and exports using SWC AST
    fn analyze_module(
        &self,
        source_code: &str,
        file_path: &str,
    ) -> Result<(Vec<String>, ModuleExports), Box<dyn std::error::Error>> {
        let _file_name = FileName::Real(PathBuf::from(file_path));

        // Determine syntax based on file extension
        let syntax = if file_path.ends_with(".ts") || file_path.ends_with(".tsx") {
            Syntax::Typescript(TsSyntax {
                tsx: file_path.ends_with(".tsx"),
                decorators: true,
                ..Default::default()
            })
        } else {
            Syntax::Es(Default::default())
        };

        let lexer = Lexer::new(
            syntax,
            EsVersion::Es2020,
            StringInput::new(source_code, BytePos(0), BytePos(source_code.len() as u32)),
            None,
        );

        let mut parser = Parser::new_from(lexer);
        let module = parser
            .parse_module()
            .map_err(|e| format!("Parse error: {e:?}"))?;

        let mut visitor = ModuleAnalyzer::new();
        module.visit_with(&mut visitor);

        Ok((visitor.dependencies, visitor.exports))
    }

    /// Transpile TypeScript to JavaScript using the existing transpiler utility
    fn transpile_module(
        &self,
        source_code: &str,
        file_path: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        // If it's already a JavaScript file, return as-is
        if file_path.ends_with(".js") || file_path.ends_with(".jsx") {
            return Ok(source_code.to_string());
        }

        // Use the existing transpiler utility with panic handling
        let transpile_result =
            transpiler::transpile(source_code.to_string(), file_path.to_string());

        match transpile_result {
            Ok(transpiled_bytes) => {
                // Convert bytes back to string
                String::from_utf8(transpiled_bytes)
                    .map_err(|e| format!("Failed to convert transpiled code to UTF-8: {e}").into())
            }
            Err(e) => {
                eprintln!("Warning: TypeScript transpilation failed for {file_path}: {e}");
                eprintln!("Falling back to original source code...");
                Ok(source_code.to_string())
            }
        }
    }

    /// Generate or retrieve obfuscated numeric module ID for a resolved path
    fn generate_module_id(&mut self, resolved_path: &str) -> String {
        // Use existing ID if we've seen this resolved path before
        if let Some(&existing_id) = self.module_id_map.get(resolved_path) {
            return existing_id.to_string();
        }

        // Generate new numeric ID
        let new_id = self.next_module_id;
        self.next_module_id += 1;

        // Store mapping from resolved path to numeric ID
        self.module_id_map.insert(resolved_path.to_string(), new_id);

        new_id.to_string()
    }

    /// Generate the final bundled code with runtime module system
    fn generate_bundle(
        &self,
        modules: &HashMap<String, BundledModule>,
        _dependency_graph: &HashMap<String, Vec<String>>,
        entry_path: &str,
        builtin_modules: &HashSet<String>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut bundle = String::new();

        // Find the entry module and get its ID
        let entry_module_id = modules
            .get(entry_path)
            .map(|m| m.id.clone())
            .unwrap_or_else(|| entry_path.to_string());

        match self.config.runtime_system {
            RuntimeSystem::CommonJS => {
                bundle.push_str(&self.generate_commonjs_runtime(builtin_modules));
                bundle.push_str(&self.generate_commonjs_modules(modules)?);
                bundle.push_str(&format!(
                    "\n// Entry point\nconst __entry = __codemod_require('{entry_module_id}');\nexport default __entry.default || __entry.transform;\n"
                ));
            }
            RuntimeSystem::Custom => {
                bundle.push_str(&self.generate_custom_runtime(builtin_modules));
                bundle.push_str(&self.generate_custom_modules(modules)?);
                bundle.push_str(&format!(
                    "\n// Entry point\nconst __entry = __codemod_require('{entry_module_id}');\nexport default __entry.default || __entry.transform;\n"
                ));
            }
        }

        Ok(bundle)
    }

    /// Generate CommonJS-style runtime
    fn generate_commonjs_runtime(&self, builtin_modules: &HashSet<String>) -> String {
        let mut runtime = include_str!("runtime_commonjs.tpl").to_string();

        // Generate top-level imports for built-in modules
        let builtin_imports = self.generate_builtin_imports(builtin_modules);
        if !builtin_imports.is_empty() {
            runtime.push('\n');
            runtime.push_str(&builtin_imports);
        }

        runtime
    }

    /// Generate custom runtime system
    fn generate_custom_runtime(&self, builtin_modules: &HashSet<String>) -> String {
        let mut runtime = include_str!("runtime_custom.tpl").to_string();

        // Generate top-level imports for built-in modules
        let builtin_imports = self.generate_builtin_imports(builtin_modules);
        if !builtin_imports.is_empty() {
            runtime.push('\n');
            runtime.push_str(&builtin_imports);
        }

        runtime
    }

    /// Generate top-level imports for built-in modules
    fn generate_builtin_imports(&self, builtin_modules: &HashSet<String>) -> String {
        let mut imports = String::new();

        for module in builtin_modules {
            let variable_name = self.get_builtin_variable_name(module);
            imports.push_str(&format!("import * as {variable_name} from '{module}';"));
            imports.push('\n');
        }

        // Register built-in modules with the runtime
        for module in builtin_modules {
            let variable_name = self.get_builtin_variable_name(module);
            imports.push_str(&format!(
                "__codemod_register_builtin('{module}', {variable_name});"
            ));
            imports.push('\n');
        }

        imports
    }

    /// Get variable name for a built-in module
    fn get_builtin_variable_name(&self, module_path: &str) -> String {
        // Convert module path to valid variable name
        match module_path {
            "fs" => "__builtin_fs".to_string(),
            "fs/promises" => "__builtin_fs_promises".to_string(),
            "path" => "__builtin_path".to_string(),
            "os" => "__builtin_os".to_string(),
            "child_process" => "__builtin_child_process".to_string(),
            "console" => "__builtin_console".to_string(),
            "process" => "__builtin_process".to_string(),
            module if module.starts_with("codemod:") => {
                format!(
                    "__builtin_{}",
                    module.replace("codemod:", "").replace("-", "_")
                )
            }
            _ => format!(
                "__builtin_{}",
                module_path
                    .replace("-", "_")
                    .replace(":", "_")
                    .replace("/", "_")
            ),
        }
    }

    /// Generate CommonJS-style module definitions
    fn generate_commonjs_modules(
        &self,
        modules: &HashMap<String, BundledModule>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut result = String::new();

        for module in modules.values() {
            let module_id = &module.id;

            // Create dependency ID mapping for this module
            let mut dependency_id_map = HashMap::new();
            for (i, resolved_dep) in module.resolved_dependencies.iter().enumerate() {
                let original_dep = &module.dependencies[i];
                if Self::is_builtin_module(original_dep) {
                    // Built-in modules keep their original import path
                    dependency_id_map.insert(original_dep.clone(), original_dep.clone());
                } else if let Some(dep_module) = modules.get(resolved_dep) {
                    dependency_id_map.insert(original_dep.clone(), dep_module.id.clone());
                }
            }

            let wrapped_code = self.wrap_module_commonjs(&module.code, &dependency_id_map)?;

            result.push_str(&format!(
                "__codemod_define('{module_id}', function(module, exports, require) {{\n{wrapped_code}\n}});\n\n"
            ));
        }

        Ok(result)
    }

    /// Generate custom module definitions
    fn generate_custom_modules(
        &self,
        modules: &HashMap<String, BundledModule>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut result = String::new();

        for module in modules.values() {
            let module_id = &module.id;

            // Create dependency ID mapping for this module
            let mut dependency_id_map = HashMap::new();
            for (i, resolved_dep) in module.resolved_dependencies.iter().enumerate() {
                let original_dep = &module.dependencies[i];
                if Self::is_builtin_module(original_dep) {
                    // Built-in modules keep their original import path
                    dependency_id_map.insert(original_dep.clone(), original_dep.clone());
                } else if let Some(dep_module) = modules.get(resolved_dep) {
                    dependency_id_map.insert(original_dep.clone(), dep_module.id.clone());
                }
            }

            let wrapped_code = self.wrap_module_custom(&module.code, &dependency_id_map)?;

            result.push_str(&format!(
                "__codemod_define('{module_id}', function(exports, require) {{\n{wrapped_code}\n}});\n\n"
            ));
        }

        Ok(result)
    }

    /// Wrap module code for CommonJS
    fn wrap_module_commonjs(
        &self,
        code: &str,
        dependency_id_map: &HashMap<String, String>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        // Transform ES6 imports/exports to CommonJS
        self.transform_to_commonjs(code, dependency_id_map)
    }

    /// Wrap module code for custom runtime
    fn wrap_module_custom(
        &self,
        code: &str,
        dependency_id_map: &HashMap<String, String>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        // Similar to CommonJS but with custom module format
        self.transform_to_commonjs(code, dependency_id_map)
    }

    /// Transform ES6 modules to CommonJS using SWC AST transformations
    fn transform_to_commonjs(
        &self,
        source_code: &str,
        dependency_id_map: &HashMap<String, String>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        // Parse the source code
        let syntax = Syntax::Es(Default::default());
        let lexer = Lexer::new(
            syntax,
            EsVersion::Es2020,
            StringInput::new(source_code, BytePos(0), BytePos(source_code.len() as u32)),
            None,
        );

        let mut parser = Parser::new_from(lexer);
        let mut module = parser
            .parse_module()
            .map_err(|e| format!("Parse error: {e:?}"))?;

        // Apply transformations
        let mut transformer = ModuleTransformer::new(dependency_id_map.clone());
        module.visit_mut_with(&mut transformer);

        // Add any remaining export assignments at the end
        for export_stmt in transformer.export_assignments {
            module.body.push(ModuleItem::Stmt(export_stmt));
        }

        let cm = Arc::new(SourceMap::default());
        let mut buf = Vec::new();
        {
            let writer = JsWriter::new(cm.clone(), "\n", &mut buf, None);
            let mut emitter = Emitter {
                cfg: Default::default(),
                comments: None,
                cm: cm.clone(),
                wr: writer,
            };
            emitter.emit_module(&module)?;
        }

        String::from_utf8(buf).map_err(|e| e.into())
    }
}

/// Result of bundling operation
pub struct BundleResult {
    /// Final bundled code
    pub code: String,
    /// All modules in the bundle
    pub modules: Vec<BundledModule>,
    /// Entry module path
    pub entry_module: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_bundler_creation() {
        let temp_dir = TempDir::new().unwrap();
        let config = BundlerConfig {
            base_dir: temp_dir.path().to_path_buf(),
            tsconfig_path: None,
            runtime_system: RuntimeSystem::CommonJS,
            source_maps: false,
        };

        let bundler = Bundler::new(config);
        assert!(bundler.is_ok());
    }

    #[test]
    fn test_module_analysis() {
        let temp_dir = TempDir::new().unwrap();
        let config = BundlerConfig {
            base_dir: temp_dir.path().to_path_buf(),
            tsconfig_path: None,
            runtime_system: RuntimeSystem::CommonJS,
            source_maps: false,
        };

        let bundler = Bundler::new(config).unwrap();

        let source = r#"
            import { foo } from './other';
            export const bar = 42;
            export default function() { return bar; }
        "#;

        let (deps, exports) = bundler.analyze_module(source, "test.js").unwrap();

        assert_eq!(deps, vec!["./other"]);
        assert!(exports.has_default);
        assert!(exports.named.contains(&"bar".to_string()));
    }
}
