use ignore::WalkBuilder;
use std::path::Path;

/// Directory walker settings shared by every codemod file enumeration: the
/// workflow engine's JSSG steps, shard planning, and the execution bridge.
///
/// Hidden files are visited, `.gitignore` / `.ignore` / global git excludes
/// are honored even outside a git repository, symlinks are not followed, and
/// ignore files in parent directories apply. Keeping this in one place means
/// every entry point selects the same files for the same target.
pub fn codemod_walk_builder(base: &Path) -> WalkBuilder {
    let mut builder = WalkBuilder::new(base);
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

#[cfg(test)]
mod tests {
    use super::codemod_walk_builder;
    use std::path::PathBuf;

    fn walk(base: &std::path::Path) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = codemod_walk_builder(base)
            .build()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
            .map(|entry| {
                entry
                    .path()
                    .strip_prefix(base)
                    .expect("inside base")
                    .to_path_buf()
            })
            .collect();
        files.sort();
        files
    }

    #[test]
    fn visits_hidden_files_and_honors_gitignore_without_a_repository() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".hidden")).expect("hidden dir");
        std::fs::create_dir_all(dir.path().join("ignored")).expect("ignored dir");
        std::fs::write(dir.path().join(".gitignore"), "ignored/\n").expect("gitignore");
        std::fs::write(dir.path().join(".hidden/h.ts"), "").expect("hidden file");
        std::fs::write(dir.path().join("ignored/i.ts"), "").expect("ignored file");
        std::fs::write(dir.path().join("kept.ts"), "").expect("kept file");

        assert_eq!(
            walk(dir.path()),
            vec![
                PathBuf::from(".gitignore"),
                PathBuf::from(".hidden/h.ts"),
                PathBuf::from("kept.ts"),
            ]
        );
    }
}
