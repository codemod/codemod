use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path};
use testing_utils::ReporterType;

const DEFAULT_TIMEOUT: u64 = 30;

/// Configuration that can be specified in test.config.json files
/// Only includes settings that are applicable per-test-case
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TestConfig {
    /// Language to process (javascript, typescript, etc.)
    pub language: Option<String>,

    /// Test timeout in seconds
    pub timeout: Option<u64>,

    /// Ignore whitespace differences in comparisons
    pub ignore_whitespace: Option<bool>,

    /// Test patterns that are expected to produce errors
    pub expect_errors: Option<Vec<String>>,

    /// Parameters to pass to the codemod
    pub params: Option<HashMap<String, serde_json::Value>>,
}

/// Merged configuration from CLI args and config files
/// Contains both global settings (from CLI only) and per-test settings
#[derive(Debug, Clone)]
pub struct ResolvedTestConfig {
    // Per-test configurable options
    pub language: Option<String>,
    pub timeout: u64,
    pub ignore_whitespace: bool,
    pub expect_errors: Vec<String>,
    pub params: Option<HashMap<String, serde_json::Value>>,

    // Global-only options (CLI args only)
    pub filter: Option<String>,
    pub update_snapshots: bool,
    pub verbose: bool,
    pub sequential: bool,
    pub max_threads: Option<usize>,
    pub fail_fast: bool,
    pub watch: bool,
    pub reporter: ReporterType,
    pub context_lines: usize,
}

impl TestConfig {
    /// Load configuration from a specific path
    /// Supports multiple config file formats and aliases in order of precedence:
    /// 1. test.config.json
    /// 2. test.config.yaml  
    /// 3. codemod-test.config.json
    /// 4. codemod-test.config.yaml
    pub fn load_from_path(path: &Path) -> Result<Option<TestConfig>> {
        let config_filenames = [
            "test.config.json",
            "test.config.yaml",
            "codemod-test.config.json",
            "codemod-test.config.yaml",
        ];

        for filename in config_filenames {
            let config_path = path.join(filename);
            if config_path.exists() {
                let content = std::fs::read_to_string(&config_path).with_context(|| {
                    format!("Failed to read config file: {}", config_path.display())
                })?;

                let config: TestConfig = if filename.ends_with(".json") {
                    serde_json::from_str(&content).with_context(|| {
                        format!(
                            "Failed to parse JSON config file: {}",
                            config_path.display()
                        )
                    })?
                } else if filename.ends_with(".yaml") {
                    serde_yaml::from_str(&content).with_context(|| {
                        format!(
                            "Failed to parse YAML config file: {}",
                            config_path.display()
                        )
                    })?
                } else {
                    return Err(anyhow::anyhow!(
                        "Unsupported config file format: {}",
                        filename
                    ));
                };

                return Ok(Some(config));
            }
        }

        Ok(None)
    }

    /// Load hierarchical configuration starting from a specific directory
    /// Walks up the directory tree collecting all test.config.json files
    /// Returns config with incremental inheritance: closer to start_path = higher precedence
    pub fn load_hierarchical(start_path: &Path, stop_path: Option<&Path>) -> Result<TestConfig> {
        let mut merged = TestConfig::default();
        let mut current_path = start_path.to_path_buf();

        // Collect configs from furthest ancestor to closest descendant
        let mut configs = Vec::new();
        loop {
            if let Some(stop_path) = stop_path {
                if current_path == *stop_path {
                    break;
                }
            }
            if let Some(config) = Self::load_from_path(&current_path)? {
                configs.push(config);
            }
            match current_path.parent() {
                Some(parent) => current_path = parent.to_path_buf(),
                None => break,
            }
        }

        // Reverse so we apply from root to leaf (leaf configs override root configs)
        configs.reverse();

        // Apply configs with proper precedence
        for config in configs {
            merged.merge(config);
        }

        Ok(merged)
    }

    /// Merge another config into this one (other takes precedence for non-None values)
    pub fn merge(&mut self, other: TestConfig) {
        if other.language.is_some() {
            self.language = other.language;
        }
        if other.timeout.is_some() {
            self.timeout = other.timeout;
        }
        if other.ignore_whitespace.is_some() {
            self.ignore_whitespace = other.ignore_whitespace;
        }
        if other.expect_errors.is_some() {
            self.expect_errors = other.expect_errors;
        }
        if other.params.is_some() {
            self.params = other.params;
        }
    }
}

impl ResolvedTestConfig {
    /// Resolve configuration from CLI args and config files
    /// per_test_config is optional - if provided, it gets merged with base_config
    pub fn resolve(
        cli_args: &super::test::Command,
        base_config: &TestConfig,
        per_test_config: Option<&TestConfig>,
    ) -> Result<Self> {
        // Merge base config with per-test config if provided
        let mut merged_config = base_config.clone();
        if let Some(test_config) = per_test_config {
            merged_config.merge(test_config.clone());
        }

        // Global settings: always come from CLI args only
        let filter = cli_args.filter.clone();
        let update_snapshots = cli_args.update_snapshots;
        let verbose = cli_args.verbose;
        let sequential = cli_args.sequential;
        let max_threads = cli_args.max_threads;
        let fail_fast = cli_args.fail_fast;
        let watch = cli_args.watch;
        let context_lines = cli_args.context_lines;
        let reporter = cli_args
            .reporter
            .parse::<ReporterType>()
            .map_err(|e| anyhow::anyhow!("Invalid reporter type: {}", e))?;

        // Per-test settings: CLI args override config files
        let language = cli_args
            .language
            .clone()
            .or_else(|| merged_config.language.clone());
        let timeout = if cli_args.timeout != DEFAULT_TIMEOUT {
            cli_args.timeout
        } else {
            merged_config.timeout.unwrap_or(DEFAULT_TIMEOUT)
        };
        let ignore_whitespace = if cli_args.ignore_whitespace {
            true
        } else {
            merged_config.ignore_whitespace.unwrap_or(false)
        };
        let expect_errors = if let Some(patterns) = &cli_args.expect_errors {
            patterns.split(',').map(|s| s.trim().to_string()).collect()
        } else {
            merged_config.expect_errors.clone().unwrap_or_default()
        };
        let params = merged_config.params.clone();

        Ok(Self {
            language,
            timeout,
            ignore_whitespace,
            expect_errors,
            filter,
            update_snapshots,
            verbose,
            sequential,
            max_threads,
            fail_fast,
            watch,
            reporter,
            context_lines,
            params,
        })
    }
}
