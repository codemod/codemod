use libtest_mimic::Arguments;
use std::str::FromStr;
use std::time::Duration;

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
        let mut args = Arguments::default();

        if let Some(filter) = &self.filter {
            args.filter = Some(filter.clone());
        }

        args.nocapture = self.verbose;

        // Handle threading - if not parallel, set to 1 thread
        if !self.parallel {
            args.test_threads = Some(1);
        } else if let Some(threads) = self.max_threads {
            args.test_threads = Some(threads);
        }

        // Map our reporter to libtest format
        args.format = Some(match self.reporter {
            ReporterType::Console => libtest_mimic::FormatSetting::Pretty,
            ReporterType::Json => libtest_mimic::FormatSetting::Json,
            ReporterType::Terse => libtest_mimic::FormatSetting::Terse,
        });

        // Set quiet mode for terse output
        if matches!(self.reporter, ReporterType::Terse) {
            args.quiet = true;
        }

        args
    }

    /// Check if tests should fail fast (stop on first failure)
    pub fn should_fail_fast(&self) -> bool {
        self.fail_fast
    }
}
