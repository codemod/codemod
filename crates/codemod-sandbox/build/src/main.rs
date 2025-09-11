use std::{
    fmt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use clap::Parser;
use tokio::{fs, io::AsyncReadExt, process::Command};

/// Configuration for the WebAssembly build process
#[derive(Parser, Debug, Clone)]
#[command(version)]
struct BuildConfig {
    /// Enable release mode compilation
    #[arg(short, long, default_value_t = false)]
    release: bool,

    /// Output directory for build artifacts
    #[arg(short, long, default_value = "./build/")]
    output: PathBuf,

    /// Enable asyncify transformation
    #[arg(long, default_value_t = false)]
    asyncify: bool,

    /// Workspace root directory
    #[arg(long)]
    workspace_root: Option<PathBuf>,

    /// Enable optimization passes
    #[arg(long, default_value_t = false)]
    optimize: bool,

    #[arg(skip)]
    manifest_data: Option<cargo_toml::Manifest>,
}

impl BuildConfig {
    fn resolve_workspace_path(&self) -> Result<PathBuf> {
        match &self.workspace_root {
            Some(path) => Ok(path.clone()),
            None => Ok(std::env::current_dir()?),
        }
    }

    fn manifest_path(&self) -> Result<PathBuf> {
        Ok(self.resolve_workspace_path()?.join("Cargo.toml"))
    }

    fn build_target_dir(&self) -> Result<PathBuf> {
        Ok(self.resolve_workspace_path()?.join("target"))
    }

    fn load_manifest(&mut self) -> Result<()> {
        let manifest = cargo_toml::Manifest::from_path(self.manifest_path()?)?;
        self.manifest_data = Some(manifest);
        Ok(())
    }
}

/// WebAssembly builder that handles the complete build pipeline
struct WasmBuilder<'a> {
    config: &'a BuildConfig,
}

impl<'a> WasmBuilder<'a> {
    fn new(config: &'a BuildConfig) -> Self {
        Self { config }
    }

    async fn execute_build_pipeline(&self) -> Result<()> {
        let package_name = "codemod-sandbox";
        let wasm_filename = format!("{package_name}.wasm");

        self.compile_wasm_package(package_name)
            .await
            .context("Failed to compile WebAssembly package")?;

        self.generate_bindings(&wasm_filename)
            .await
            .context("Failed to generate bindings")?;

        // Execute post-processing tasks concurrently
        let js_file = format!("{package_name}_bg.js");
        let wasm_file = self
            .config
            .build_target_dir()?
            .join("wasm-bindgen")
            .join(format!("{package_name}_bg.wasm"));

        tokio::try_join!(
            self.transform_js_bindings(&js_file),
            self.optimize_wasm_binary(&wasm_file)
        )?;

        Ok(())
    }

    async fn compile_wasm_package(&self, package: &str) -> Result<()> {
        let mut cmd_args = vec![
            "build",
            "--target",
            "wasm32-wasip1",
            "-p",
            package,
            "--features",
            "wasm",
            "--no-default-features",
        ];

        if self.config.release {
            cmd_args.push("--release");
        }

        let mut child = Command::new("cargo")
            .args(cmd_args)
            .spawn()
            .context("Failed to spawn cargo build")?;

        let status = child.wait().await.context("Cargo build process failed")?;

        if !status.success() {
            anyhow::bail!("Cargo build failed with status: {}", status);
        }

        Ok(())
    }

    async fn generate_bindings(&self, wasm_filename: &str) -> Result<()> {
        let build_profile = if self.config.release {
            "release"
        } else {
            "debug"
        };
        let wasm_input = self
            .config
            .build_target_dir()?
            .join("wasm32-wasip1")
            .join(build_profile)
            .join(wasm_filename);

        let mut generator = wasm_bindgen_cli_support::Bindgen::new();
        generator
            .input_path(wasm_input)
            .keep_debug(!self.config.release)
            .bundler(true)?
            .emit_start(true)
            .omit_default_module_path(false);

        let output_dir = self.config.build_target_dir()?.join("wasm-bindgen");
        fs::create_dir_all(&output_dir)
            .await
            .context("Failed to create bindgen output directory")?;

        generator
            .generate(&output_dir)
            .context("Bindgen generation failed")?;

        Ok(())
    }

    async fn transform_js_bindings(&self, js_filename: &str) -> Result<()> {
        let js_path = self
            .config
            .build_target_dir()?
            .join("wasm-bindgen")
            .join(js_filename);

        let source_content = fs::read_to_string(&js_path)
            .await
            .context("Failed to read generated JS file")?;

        let transformed = JsTransformer::new(&source_content)
            .wrap_exports_in_factory()
            .context("Failed to transform JS exports")?;

        fs::write(&js_path, transformed)
            .await
            .context("Failed to write transformed JS file")?;

        Ok(())
    }

    async fn optimize_wasm_binary(&self, wasm_path: &Path) -> Result<()> {
        let mut wasm_data = Vec::new();
        fs::File::open(wasm_path)
            .await
            .context("Failed to open WASM file")?
            .read_to_end(&mut wasm_data)
            .await
            .context("Failed to read WASM file")?;

        println!("Original size: {}", ByteSize(wasm_data.len()));

        let processed_data = if self.config.optimize {
            WasmOptimizer::new(self.config).process(wasm_data)?
        } else {
            wasm_data
        };

        println!("Processed size: {}", ByteSize(processed_data.len()));

        fs::write(wasm_path, &processed_data)
            .await
            .context("Failed to write optimized WASM")?;

        Ok(())
    }
}

/// Transforms JavaScript bindings to wrap exports in a factory function
struct JsTransformer {
    content: String,
}

impl JsTransformer {
    fn new(content: &str) -> Self {
        Self {
            content: content.to_string(),
        }
    }

    fn wrap_exports_in_factory(self) -> Result<String> {
        let lines: Vec<&str> = self.content.lines().collect();
        let exported_symbols = self.extract_export_names(&lines)?;

        let mut transformed_lines: Vec<String> = lines
            .iter()
            .map(|line| line.trim().trim_start_matches("export").to_string())
            .collect();

        // Find insertion point after imports
        let insert_idx = transformed_lines
            .iter()
            .position(|line| !line.trim().starts_with("import"))
            .unwrap_or(0);

        // Insert factory function header
        transformed_lines.insert(insert_idx, String::new());
        transformed_lines.insert(
            insert_idx + 1,
            "export const RAW_WASM = Symbol();".to_string(),
        );
        transformed_lines.insert(insert_idx + 2, "export default function() {".to_string());

        // Add return statement with exports
        transformed_lines.push("return {".to_string());
        transformed_lines.push("  [RAW_WASM]: wasm,".to_string());

        for symbol in exported_symbols {
            transformed_lines.push(format!("  {symbol},"));
        }

        transformed_lines.push("  Parser: Parser,".to_string());
        transformed_lines.push("  Language: Language,".to_string());

        transformed_lines.push("};".to_string());
        transformed_lines.push("}".to_string());

        Ok(transformed_lines.join("\n"))
    }

    fn extract_export_names(&self, lines: &[&str]) -> Result<Vec<String>> {
        let export_lines: Vec<_> = lines
            .iter()
            .filter(|line| line.trim().starts_with("export"))
            .collect();

        let mut symbols = Vec::new();

        for line in export_lines {
            let trimmed = line.trim();

            if trimmed.starts_with("export async function")
                || trimmed.starts_with("export function")
            {
                let symbol = trimmed
                    .split_whitespace()
                    .find(|part| part.contains('('))
                    .context("Function export missing parentheses")?
                    .split('(')
                    .next()
                    .context("Invalid function syntax")?;
                symbols.push(symbol.to_string());
            } else if trimmed.contains("const") {
                let symbol = trimmed
                    .split('=')
                    .next()
                    .context("Const export missing assignment")?
                    .split_whitespace()
                    .last()
                    .context("Invalid const declaration")?;
                symbols.push(symbol.to_string());
            } else {
                anyhow::bail!("Unrecognized export pattern: {}", line);
            }
        }

        Ok(symbols)
    }
}

/// Handles WebAssembly optimization using binaryen
struct WasmOptimizer<'a> {
    #[cfg_attr(not(feature = "optimize"), allow(dead_code))]
    config: &'a BuildConfig,
}

impl<'a> WasmOptimizer<'a> {
    fn new(config: &'a BuildConfig) -> Self {
        Self { config }
    }

    fn process(&self, wasm_data: Vec<u8>) -> Result<Vec<u8>> {
        #[cfg(feature = "optimize")]
        {
            use binaryen::binaryen_sys;

            let mut data = wasm_data;
            let module = unsafe {
                let raw_module = binaryen_sys::BinaryenModuleReadWithFeatures(
                    data.as_mut_ptr() as *mut i8,
                    data.len(),
                    binaryen_sys::BinaryenFeatureMVP()
                        | binaryen_sys::BinaryenFeatureReferenceTypes(),
                );
                binaryen::Module::from_raw(raw_module)
            };

            let opt_level = self.determine_optimization_level();
            let passes = self.collect_optimization_passes();

            let config = binaryen::CodegenConfig {
                shrink_level: 0,
                optimization_level: opt_level,
                debug_info: !self.config.release,
            };

            let mut opt_module = module;
            opt_module
                .run_optimization_passes(passes, &config)
                .map_err(|_| anyhow::anyhow!("Optimization process failed"))?;

            Ok(opt_module.write())
        }
        #[cfg(not(feature = "optimize"))]
        {
            // Return the original WASM data without optimization
            println!("Binaryen optimization disabled - returning original WASM");
            Ok(wasm_data)
        }
    }

    #[cfg(feature = "optimize")]
    fn determine_optimization_level(&self) -> u32 {
        let base_level = if self.config.release { 3 } else { 0 };

        if self.config.asyncify {
            // Asyncify requires at least optimization level 1
            base_level.max(1)
        } else {
            base_level
        }
    }

    #[cfg(feature = "optimize")]
    fn collect_optimization_passes(&self) -> Vec<&'static str> {
        let mut passes = Vec::new();

        if self.config.asyncify {
            passes.push("asyncify");
        }

        if self.config.release {
            passes.push("strip-debug");
        }

        passes
    }
}

/// Utility for displaying file sizes in human-readable format
struct ByteSize(usize);

impl fmt::Display for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const MB: f64 = 1024.0 * 1024.0;
        const KB: f64 = 1024.0;

        let size = self.0 as f64;

        if size >= MB {
            write!(f, "{:.2}MB", size / MB)
        } else if size >= KB {
            write!(f, "{:.2}KB", size / KB)
        } else {
            write!(f, "{}B", self.0)
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut config = BuildConfig::parse();
    config
        .load_manifest()
        .context("Failed to load cargo manifest")?;

    let builder = WasmBuilder::new(&config);
    builder
        .execute_build_pipeline()
        .await
        .context("Build pipeline failed")?;

    Ok(())
}
