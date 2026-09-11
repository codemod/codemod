//! Path containment used on both directions of the worker boundary.

use std::path::Path;

use butterflow_execution_bridge::paths::{canonical_root, normalize_output, resolve_source};

fn root() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("src/nested")).expect("mkdir");
    std::fs::write(dir.path().join("src/a.ts"), "a").expect("write");
    let canonical = canonical_root(dir.path(), "target root").expect("canonical");
    (dir, canonical)
}

#[test]
fn sources_resolve_beneath_the_root_whether_or_not_they_exist() {
    let (_dir, root) = root();
    assert_eq!(
        resolve_source(&root, "src/a.ts", "path").expect("existing"),
        root.join("src/a.ts")
    );
    assert_eq!(
        resolve_source(&root, "src/nested/new.ts", "path").expect("new file"),
        root.join("src/nested/new.ts")
    );
    assert_eq!(
        resolve_source(&root, "brand/new/dir/x.ts", "path").expect("new dirs"),
        root.join("brand/new/dir/x.ts")
    );
    for bad in ["../x.ts", "/x.ts", "src/../../x.ts", "C:\\x.ts", "", "  "] {
        assert!(resolve_source(&root, bad, "path").is_err(), "{bad:?}");
    }
}

#[test]
fn outputs_normalize_to_root_relative_wire_paths() {
    let (_dir, root) = root();
    assert_eq!(
        normalize_output(&root, &root.join("src/a.ts"), "out").expect("absolute inside"),
        "src/a.ts"
    );
    assert_eq!(
        normalize_output(&root, Path::new("src/new/b.ts"), "out").expect("relative"),
        "src/new/b.ts"
    );
    assert_eq!(
        normalize_output(&root, &root.join("./src/./a.ts"), "out").expect("dots"),
        "src/a.ts"
    );
    for bad in [
        root.join("../escape.ts"),
        root.join("src/../../escape.ts"),
        std::env::temp_dir().join("elsewhere.ts"),
        std::path::PathBuf::from("../escape.ts"),
        root.clone(),
        std::path::PathBuf::from(""),
    ] {
        assert!(
            normalize_output(&root, &bad, "out").is_err(),
            "{}",
            bad.display()
        );
    }
}

#[cfg(unix)]
#[test]
fn symlinks_are_resolved_through_the_nearest_existing_ancestor() {
    let (_dir, root) = root();
    let outside = tempfile::tempdir().expect("outside");
    std::fs::create_dir_all(outside.path().join("dir")).expect("outside dir");
    std::fs::write(outside.path().join("secret.ts"), "s").expect("secret");
    std::os::unix::fs::symlink(outside.path().join("dir"), root.join("linkdir")).expect("link");
    std::os::unix::fs::symlink(outside.path().join("secret.ts"), root.join("link.ts"))
        .expect("link file");
    std::os::unix::fs::symlink(&root, outside.path().join("root-link")).expect("root link");

    // Existing symlink to a file outside, new file under a symlinked dir outside.
    assert!(resolve_source(&root, "link.ts", "path").is_err());
    assert!(resolve_source(&root, "linkdir/new.ts", "path").is_err());
    assert!(normalize_output(&root, &root.join("linkdir/new.ts"), "out").is_err());
    assert!(normalize_output(&root, Path::new("link.ts"), "out").is_err());

    // A non-canonical absolute spelling of a path inside the root is accepted
    // and normalized to the canonical relative form.
    assert_eq!(
        normalize_output(&root, &outside.path().join("root-link/src/new.ts"), "out")
            .expect("through link into root"),
        "src/new.ts"
    );
}
