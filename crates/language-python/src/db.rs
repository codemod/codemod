//! Database wrapper for ty_ide integration.
//!
//! This module provides functions to create and use ty_project's database
//! for semantic analysis operations. The database is created per-operation
//! to ensure thread safety, but file contents are cached to support cross-file
//! analysis.

use ruff_db::files::{system_path_to_file, File, FileRootKind, Files};
use ruff_db::system::{System, SystemPath, SystemPathBuf, TestSystem};
use ruff_db::vendored::VendoredFileSystem;
use ruff_db::Db as SourceDb;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use ty_project::{Db, Project, ProjectMetadata};
use ty_python_semantic::lint::{LintRegistry, RuleSelection};
use ty_python_semantic::{
    Program, ProgramSettings, PythonPlatform, PythonVersionWithSource, SearchPathSettings,
};

/// Database for Python semantic analysis.
///
/// This database wraps ty_project's infrastructure and provides
/// access to ty_ide's goto-definition and find-references functionality.
#[salsa::db]
#[derive(Clone)]
pub struct PythonSemanticDb {
    storage: salsa::Storage<Self>,
    files: Files,
    system: TestSystem,
    vendored: VendoredFileSystem,
    project: Option<Project>,
}

impl PythonSemanticDb {
    /// Create a new database for a workspace with given files.
    pub fn new_with_files(
        workspace_root: &Path,
        file_contents: &HashMap<PathBuf, String>,
    ) -> anyhow::Result<Self> {
        let root = SystemPathBuf::from(workspace_root.to_string_lossy().to_string());
        let metadata = ProjectMetadata::new("project".into(), root.clone());

        let system = TestSystem::default();

        system
            .as_writable()
            .unwrap()
            .create_directory_all(&root)
            .ok(); // Ignore error if directory exists

        // write all files to the in-memory filesystem
        for (path, content) in file_contents {
            let path_str = path.to_string_lossy().to_string();
            let system_path = SystemPath::new(&path_str);

            // create parent directory if needed
            if let Some(parent) = system_path.parent() {
                system
                    .as_writable()
                    .unwrap()
                    .create_directory_all(parent)
                    .ok();
            }

            system
                .as_writable()
                .unwrap()
                .write_file(system_path, content)
                .ok();
        }

        let mut db = Self {
            storage: salsa::Storage::new(None),
            system,
            vendored: ty_vendored::file_system().clone(),
            files: Files::default(),
            project: None,
        };

        let project = Project::from_metadata(&db, metadata)?;
        db.project = Some(project);

        // Initialize the program
        let search_paths = SearchPathSettings::new(vec![root.clone()])
            .to_search_paths(db.system(), db.vendored())
            .expect("Valid search path settings");

        Program::from_settings(
            &db,
            ProgramSettings {
                python_version: PythonVersionWithSource::default(),
                python_platform: PythonPlatform::default(),
                search_paths,
            },
        );

        db.files().try_add_root(&db, &root, FileRootKind::Project);

        Ok(db)
    }

    /// Get a file handle if it exists.
    pub fn get_file(&self, path: &Path) -> Option<File> {
        let path_str = path.to_string_lossy().to_string();
        let system_path = SystemPath::new(&path_str);
        system_path_to_file(self, system_path).ok()
    }
}

#[salsa::db]
impl SourceDb for PythonSemanticDb {
    fn vendored(&self) -> &VendoredFileSystem {
        &self.vendored
    }

    fn system(&self) -> &dyn System {
        &self.system
    }

    fn files(&self) -> &Files {
        &self.files
    }

    fn python_version(&self) -> ruff_python_ast::PythonVersion {
        Program::get(self).python_version(self)
    }
}

#[salsa::db]
impl ty_python_semantic::Db for PythonSemanticDb {
    fn should_check_file(&self, file: File) -> bool {
        !file.path(self).is_vendored_path()
    }

    fn rule_selection(&self, _file: File) -> &RuleSelection {
        self.project().rules(self)
    }

    fn lint_registry(&self) -> &LintRegistry {
        ty_python_semantic::default_lint_registry()
    }
}

#[salsa::db]
impl Db for PythonSemanticDb {
    fn project(&self) -> Project {
        self.project.unwrap()
    }

    fn dyn_clone(&self) -> Box<dyn Db> {
        Box::new(self.clone())
    }
}

#[salsa::db]
impl salsa::Database for PythonSemanticDb {}

/// Create a database with multiple files for cross-file analysis.
///
/// This function creates a database with all provided files, suitable
/// for workspace-scope analysis where cross-file references are needed.
pub fn create_db_with_files(
    workspace_root: &Path,
    file_contents: &HashMap<PathBuf, String>,
) -> anyhow::Result<PythonSemanticDb> {
    PythonSemanticDb::new_with_files(workspace_root, file_contents)
}
