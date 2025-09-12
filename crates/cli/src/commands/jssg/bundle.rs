use crate::utils::rolldown_bundler::{RolldownBundler, RolldownBundlerConfig};
use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
pub struct Command {
    /// Path to the entry JavaScript/TypeScript file to bundle
    pub entry_path: PathBuf,

    /// Output file path for the bundle (optional, defaults to stdout)
    #[arg(long, short)]
    pub output: Option<PathBuf>,

    /// Base directory for module resolution (defaults to entry file's directory)
    #[arg(long)]
    pub base_dir: Option<PathBuf>,

    /// Enable source maps
    #[arg(long)]
    pub source_maps: bool,

    /// Verbose output
    #[arg(long, short)]
    pub verbose: bool,
}

impl Command {
    pub async fn run(self) -> Result<()> {
        let entry_path = self.entry_path.canonicalize().map_err(|e| {
            anyhow::anyhow!(
                "Failed to resolve entry path '{}': {}",
                self.entry_path.display(),
                e
            )
        })?;

        if self.verbose {
            eprintln!("📦 Bundling entry point: {}", entry_path.display());
        }

        // Determine base directory
        let base_dir = if let Some(base_dir) = self.base_dir {
            base_dir.canonicalize().map_err(|e| {
                anyhow::anyhow!(
                    "Failed to resolve base directory '{}': {}",
                    base_dir.display(),
                    e
                )
            })?
        } else {
            entry_path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("Entry path has no parent directory"))?
                .to_path_buf()
        };

        if self.verbose {
            eprintln!("📁 Base directory: {}", base_dir.display());
            eprintln!("🗺️  Source maps: {}", self.source_maps);
        }

        // Configure rolldown bundler
        let config = RolldownBundlerConfig {
            entry_path: entry_path.clone(),
            base_dir: Some(base_dir),
            output_path: self.output.clone(),
            source_maps: self.source_maps,
        };

        // Create and run rolldown bundler
        let bundler = RolldownBundler::new(config);
        let result = bundler
            .bundle()
            .await
            .map_err(|e| anyhow::anyhow!("Rolldown bundling failed: {}", e))?;

        if self.verbose {
            eprintln!("✅ Bundle created successfully with rolldown!");
            eprintln!("   📏 Bundle size: {} bytes", result.code.len());
        }

        // Output the bundle
        if self.output.is_some() {
            if self.verbose {
                eprintln!(
                    "💾 Bundle written to: {}",
                    self.output.as_ref().unwrap().display()
                );
            }
        } else {
            // Output to stdout
            println!("{}", result.code);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_bundle_command_creation() {
        let temp_dir = TempDir::new().unwrap();
        let entry_path = temp_dir.path().join("index.js");
        fs::write(&entry_path, "console.log('Hello, rolldown world!');").unwrap();

        let command = Command {
            entry_path: entry_path.clone(),
            output: None,
            base_dir: None,
            source_maps: false,
            verbose: false,
        };

        // Should not panic when creating the command
        assert_eq!(command.entry_path, entry_path);
        assert!(!command.source_maps);
    }
}
