use libtest_mimic::Arguments;
use std::str::FromStr;
use std::time::Duration;

/// Test comparison strictness level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Strictness {
    /// Strict string equality (default behavior).
    #[default]
    Strict,
    /// Compare Concrete Syntax Trees (CSTs) - includes all tokens and whitespace.
    Cst,
    /// Compare Abstract Syntax Trees (ASTs) - ignores formatting and whitespace.
    Ast,
    /// Loose AST comparison - ignores ordering of certain children (e.g., object members).
    Loose,
}

impl FromStr for Strictness {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "strict" => Ok(Self::Strict),
            "cst" => Ok(Self::Cst),
            "ast" => Ok(Self::Ast),
            "loose" => Ok(Self::Loose),
            _ => Err(format!(
                "Invalid strictness level: '{}'. Valid options are: strict, cst, ast, loose",
                s
            )),
        }
    }
}

impl std::fmt::Display for Strictness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Strict => write!(f, "strict"),
            Self::Cst => write!(f, "cst"),
            Self::Ast => write!(f, "ast"),
            Self::Loose => write!(f, "loose"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TestOptions {
    pub filter: Option<String>,
    pub update_snapshots: bool,
    pub verbose: bool,
    pub parallel: bool,
    pub max_threads: Option<usize>,
    pub fail_fast: bool,
    pub watch: bool,
    pub reporter: ReporterType,
    pub timeout: Duration,
    pub ignore_whitespace: bool,
    pub context_lines: usize,
    pub expect_errors: Vec<String>,
    pub strictness: Strictness,
    pub language: Option<String>,
    pub expected_extension: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ReporterType {
    Console,
    Json,
    Terse,
}

impl FromStr for ReporterType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "console" => Ok(ReporterType::Console),
            "json" => Ok(ReporterType::Json),
            "terse" => Ok(ReporterType::Terse),
            _ => Err(format!(
                "Invalid reporter type: {s}. Valid options: console, json, terse"
            )),
        }
    }
}

impl TestOptions {
    pub fn to_libtest_args(&self) -> Arguments {
        Arguments {
            filter: self.filter.clone(),
            nocapture: self.verbose,
            test_threads: if !self.parallel {
                Some(1)
            } else {
                self.max_threads
            },
            format: Some(match self.reporter {
                ReporterType::Console => libtest_mimic::FormatSetting::Pretty,
                ReporterType::Json => libtest_mimic::FormatSetting::Json,
                ReporterType::Terse => libtest_mimic::FormatSetting::Terse,
            }),
            quiet: matches!(self.reporter, ReporterType::Terse),
            ..Default::default()
        }
    }

    /// Check if tests should fail fast (stop on first failure)
    pub fn should_fail_fast(&self) -> bool {
        self.fail_fast
    }
}
