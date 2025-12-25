use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_sandbox::sandbox::engine::language_data::get_extensions_for_language;
use ignore::{
    overrides::{Override, OverrideBuilder},
    WalkBuilder, WalkState,
};
use std::{
    collections::HashSet,
    error::Error,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use uuid::Uuid;

type PreRunCallbackFn = Box<dyn Fn(&Path, bool, &CodemodExecutionConfig) + Send + Sync>;

#[derive(Clone)]
pub struct PreRunCallback {
    pub callback: Arc<PreRunCallbackFn>,
}

type ProgressCallbackFn = Box<dyn Fn(&str, &str, &str, Option<&u64>, &u64) + Send + Sync>;

#[derive(Clone)]
pub struct ProgressCallback {
    pub callback: Arc<ProgressCallbackFn>,
}

/// Shared execution context to minimize Arc cloning in parallel processing
struct SharedExecutionContext<'a, F>
where
    F: Fn(&Path, &CodemodExecutionConfig) + Send + Sync,
{
    task_id: Arc<str>,
    progress_callback: Arc<Option<ProgressCallback>>,
    callback: Arc<F>,
    config: &'a CodemodExecutionConfig,
    processed_count: Arc<AtomicU64>,
    total_files: u64,
}

#[derive(Clone)]
pub struct CodemodExecutionConfig {
    /// Callback to run before the codemod execution
    pub pre_run_callback: Option<PreRunCallback>,
    /// Callback to report progress
    pub progress_callback: Arc<Option<ProgressCallback>>,
    /// Path to the target file or directory
    pub target_path: Option<PathBuf>,
    /// Path to the base directory relative to the target path
    pub base_path: Option<PathBuf>,
    /// Globs to include
    pub include_globs: Option<Vec<String>>,
    /// Globs to exclude
    pub exclude_globs: Option<Vec<String>>,
    /// Dry run mode
    pub dry_run: bool,
    /// Language
    pub languages: Option<Vec<String>>,
    /// Number of threads to use
    pub threads: Option<usize>,
    /// Capabilities
    pub capabilities: Option<HashSet<LlrtSupportedModules>>,
}

impl CodemodExecutionConfig {
    /// Execute the codemod by iterating through files and calling the provided callback
    pub fn execute<F>(&self, callback: F) -> Result<(), Box<dyn Error>>
    where
        F: Fn(&Path, &CodemodExecutionConfig) + Send + Sync,
    {
        self.execute_with_task_id(&Uuid::new_v4(), callback)
    }

    /// Execute the codemod with a specific task ID for progress tracking
    pub fn execute_with_task_id<F>(&self, task_id: &Uuid, callback: F) -> Result<(), Box<dyn Error>>
    where
        F: Fn(&Path, &CodemodExecutionConfig) + Send + Sync,
    {
        let search_base = self.get_search_base()?;

        if let Some(ref pre_run_cb) = self.pre_run_callback {
            (pre_run_cb.callback)(&search_base, !self.dry_run, self);
        }

        let globs = self.build_globs(&search_base)?;

        let total_files = self.count_files(&search_base, &globs)?;

        if let Some(ref progress_cb) = self.progress_callback.as_ref() {
            (progress_cb.callback)(
                task_id.to_string().as_str(),
                "start",
                "counting",
                Some(&total_files),
                &0,
            );
        }

        let num_threads = self.threads.unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map_or(1, |n| n.get())
                .min(12)
        });

        let walker = self
            .create_walk_builder(&search_base, globs)
            .threads(num_threads)
            .build_parallel();

        let shared_context = Arc::new(SharedExecutionContext {
            task_id: Arc::from(task_id.to_string().as_str()),
            progress_callback: self.progress_callback.clone(),
            callback: Arc::new(callback),
            config: self,
            processed_count: Arc::new(AtomicU64::new(0)),
            total_files,
        });

        walker.run(|| {
            let ctx = Arc::clone(&shared_context);

            Box::new(move |entry| match entry {
                Ok(dir_entry) => {
                    let file_path = dir_entry.path();

                    if dir_entry.file_type().is_some_and(|ft| ft.is_file()) {
                        if let Some(ref progress_cb) = ctx.progress_callback.as_ref() {
                            let file_path_str = file_path.to_string_lossy();
                            (progress_cb.callback)(
                                &ctx.task_id,
                                &file_path_str,
                                "processing",
                                Some(&ctx.total_files),
                                &ctx.processed_count.load(Ordering::Relaxed),
                            );
                        }

                        (ctx.callback)(file_path, ctx.config);

                        let current_count = ctx.processed_count.fetch_add(1, Ordering::Relaxed);

                        if let Some(ref progress_cb) = ctx.progress_callback.as_ref() {
                            (progress_cb.callback)(
                                &ctx.task_id,
                                "",
                                "increment",
                                Some(&ctx.total_files),
                                &(current_count + 1),
                            );
                        }
                    }
                    WalkState::Continue
                }
                Err(err) => {
                    eprintln!("Walk error: {err}");
                    WalkState::Continue
                }
            })
        });

        if let Some(ref progress_cb) = self.progress_callback.as_ref() {
            let final_count = shared_context.processed_count.load(Ordering::Relaxed);
            (progress_cb.callback)(
                task_id.to_string().as_str(),
                "",
                "finish",
                Some(&total_files),
                &final_count,
            );
        }

        Ok(())
    }

    /// Count total files that will be processed
    fn count_files(&self, search_base: &Path, globs: &Option<Override>) -> Result<u64, String> {
        let walker = self
            .create_walk_builder(search_base, globs.clone())
            .threads(1)
            .build();

        let mut count = 0u64;
        for entry in walker {
            match entry {
                Ok(dir_entry) => {
                    if dir_entry.file_type().is_some_and(|ft| ft.is_file()) {
                        count += 1;
                    }
                }
                Err(_) => {
                    continue;
                }
            }
        }

        Ok(count)
    }

    /// Collect all files that will be processed into a Vec
    /// This is useful for pre-processing files (e.g., semantic analysis indexing)
    pub fn collect_files(&self) -> Vec<PathBuf> {
        let search_base = match self.get_search_base() {
            Ok(base) => base,
            Err(_) => return Vec::new(),
        };

        let globs = match self.build_globs(&search_base) {
            Ok(globs) => globs,
            Err(_) => return Vec::new(),
        };

        let walker = self
            .create_walk_builder(&search_base, globs)
            .threads(1)
            .build();

        let mut files = Vec::new();
        for dir_entry in walker.flatten() {
            if dir_entry.file_type().is_some_and(|ft| ft.is_file()) {
                files.push(dir_entry.path().to_path_buf());
            }
        }

        files
    }

    /// Create a configured WalkBuilder with all the standard settings
    fn create_walk_builder(&self, base_path: &Path, overrides: Option<Override>) -> WalkBuilder {
        let mut builder = WalkBuilder::new(base_path);

        if let Some(overrides) = overrides {
            builder.overrides(overrides);
        }

        builder
            .follow_links(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .require_git(false)
            .parents(true)
            .ignore(true)
            .hidden(false);

        builder
    }

    /// Get the search base path by combining target_path and base_path
    fn get_search_base(&self) -> Result<PathBuf, String> {
        let target = self
            .target_path
            .as_ref()
            .ok_or_else(|| "target_path is required".to_string())?;

        if let Some(base) = &self.base_path {
            if base.is_absolute() {
                Err(format!("base_path is absolute: {}", base.display()))
            } else {
                Ok(target.join(base))
            }
        } else {
            Ok(target.clone())
        }
    }

    /// Build glob overrides for include/exclude patterns
    fn build_globs(&self, base_path: &Path) -> Result<Option<Override>, String> {
        let mut builder = OverrideBuilder::new(base_path);
        let mut has_patterns = false;

        if self.include_globs.is_none()
            && self
                .languages
                .as_ref()
                .is_some_and(|langs| !langs.is_empty())
        {
            for language in self.languages.as_ref().unwrap() {
                for extension in get_extensions_for_language(language.parse().unwrap()) {
                    builder
                        .add(format!("**/*{extension}").as_str())
                        .map_err(|e| format!("Failed to add language pattern: {e}"))?;
                    has_patterns = true;
                }
            }
        }

        if let Some(ref include_globs) = self.include_globs {
            for glob in include_globs {
                builder
                    .add(glob)
                    .map_err(|e| format!("Invalid include glob '{glob}': {e}"))?;
                has_patterns = true;
            }
        }

        if let Some(ref exclude_globs) = self.exclude_globs {
            for glob in exclude_globs {
                let exclude_pattern = if glob.starts_with('!') {
                    glob.to_string()
                } else {
                    format!("!{glob}")
                };
                builder
                    .add(&exclude_pattern)
                    .map_err(|e| format!("Invalid exclude glob '{exclude_pattern}': {e}"))?;
                has_patterns = true;
            }
        }

        if has_patterns {
            Ok(Some(builder.build().map_err(|e| {
                format!("Failed to build glob overrides: {e}")
            })?))
        } else {
            Ok(None)
        }
    }
}
