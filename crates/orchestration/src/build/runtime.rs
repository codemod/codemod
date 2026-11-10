//! The portable runtime's sources, built into this crate and written once
//! per content hash under a private cache directory, because rolldown
//! bundles from files.
use std::{fs, io, path::Path};

use crate::build::{sha256, Runtime};

include!(concat!(env!("OUT_DIR"), "/runtime_files.rs"));

impl Runtime {
    /// The runtime this binary was built with, materialized under `cache`
    /// (which must be private to this user, see [`crate::build::private_directory`]).
    pub fn embedded(cache: &Path) -> io::Result<Runtime> {
        let parts: Vec<&[u8]> = RUNTIME_FILES
            .iter()
            .flat_map(|(path, content)| [path.as_bytes(), *content])
            .collect();
        let directory = cache.join(sha256(&parts));
        if !directory.is_dir() {
            let temporary = cache.join(format!(
                "{}.{}.tmp",
                directory.file_name().unwrap().to_string_lossy(),
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&temporary);
            for (path, content) in RUNTIME_FILES {
                let file = temporary.join(path);
                fs::create_dir_all(file.parent().unwrap())?;
                fs::write(file, content)?;
            }
            // A concurrent run may have won the rename; its copy is identical.
            if fs::rename(&temporary, &directory).is_err() && directory.is_dir() {
                let _ = fs::remove_dir_all(&temporary);
            }
        }
        Ok(Runtime {
            src: directory.canonicalize()?,
        })
    }
}
