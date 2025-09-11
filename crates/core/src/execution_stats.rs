use std::{
    fmt,
    sync::atomic::{AtomicUsize, Ordering},
};

/// Statistics about the execution results
#[derive(Debug, Default)]
pub struct ExecutionStats {
    pub files_modified: AtomicUsize,
    pub files_unmodified: AtomicUsize,
    pub files_with_errors: AtomicUsize,
}

impl ExecutionStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Total number of files processed
    pub fn total_files(&self) -> usize {
        self.files_modified.load(Ordering::Relaxed)
            + self.files_unmodified.load(Ordering::Relaxed)
            + self.files_with_errors.load(Ordering::Relaxed)
    }

    /// Returns true if any files were processed successfully (modified or unmodified)
    pub fn has_successful_files(&self) -> bool {
        self.files_modified.load(Ordering::Relaxed) > 0
            || self.files_unmodified.load(Ordering::Relaxed) > 0
    }

    /// Returns true if any files had errors during processing
    pub fn has_errors(&self) -> bool {
        self.files_with_errors.load(Ordering::Relaxed) > 0
    }

    /// Returns the success rate as a percentage (0.0 to 1.0)
    pub fn success_rate(&self) -> f64 {
        let total = self.total_files();
        if total == 0 {
            0.0
        } else {
            (self.files_modified.load(Ordering::Relaxed)
                + self.files_unmodified.load(Ordering::Relaxed)) as f64
                / total as f64
        }
    }
}

impl fmt::Display for ExecutionStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Execution Summary: {} files processed ({} modified, {} unmodified, {} errors)",
            self.total_files(),
            self.files_modified.load(Ordering::Relaxed),
            self.files_unmodified.load(Ordering::Relaxed),
            self.files_with_errors.load(Ordering::Relaxed)
        )
    }
}
